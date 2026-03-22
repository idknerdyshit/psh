use std::collections::HashMap;

use secrecy::ExposeSecret;
use secrecy::SecretString;
use zbus::Connection;
use zbus::zvariant::{OwnedValue, Value};

use psh_core::Result;

/// A polkit subject: `(kind, details)` — e.g. `("unix-session", {"session-id": "42"})`.
pub type Subject = (String, HashMap<String, OwnedValue>);

/// A polkit identity: `(kind, details)` — e.g. `("unix-user", {"uid": 1000})`.
pub type Identity = (String, HashMap<String, OwnedValue>);

/// Agent object path where we serve the authentication agent interface.
const AGENT_OBJECT_PATH: &str = "/org/freedesktop/PolicyKit1/AuthenticationAgent";

/// Path to the polkit authentication helper binary.
const POLKIT_HELPER: &str = "/usr/lib/polkit-1/polkit-agent-helper-1";

// ---------------------------------------------------------------------------
// Polkit Authority D-Bus proxy
// ---------------------------------------------------------------------------

#[zbus::proxy(
    interface = "org.freedesktop.PolicyKit1.Authority",
    default_service = "org.freedesktop.PolicyKit1",
    default_path = "/org/freedesktop/PolicyKit1/Authority"
)]
trait Authority {
    /// Register this process as a polkit authentication agent for the given subject.
    fn register_authentication_agent(
        &self,
        subject: &Subject,
        locale: &str,
        object_path: &str,
    ) -> zbus::Result<()>;

    /// Unregister this process as a polkit authentication agent.
    fn unregister_authentication_agent(
        &self,
        subject: &Subject,
        object_path: &str,
    ) -> zbus::Result<()>;

    /// Tell polkitd that authentication for `cookie` succeeded for `identity`.
    fn authentication_agent_response2(
        &self,
        uid: u32,
        cookie: &str,
        identity: &Identity,
    ) -> zbus::Result<()>;
}

// ---------------------------------------------------------------------------
// Session subject detection
// ---------------------------------------------------------------------------

/// Build the session subject for `RegisterAuthenticationAgent`.
///
/// Reads `$XDG_SESSION_ID` (set by logind on systemd-based systems). Falls back
/// to reading `/proc/self/sessionid` if the env var is missing.
pub fn detect_session_subject() -> Result<Subject> {
    let session_id = std::env::var("XDG_SESSION_ID")
        .or_else(|_| std::fs::read_to_string("/proc/self/sessionid").map(|s| s.trim().to_string()))
        .map_err(|_| {
            psh_core::PshError::Other(
                "cannot determine session id: $XDG_SESSION_ID not set and /proc/self/sessionid unreadable".into(),
            )
        })?;

    // /proc/self/sessionid may contain "unset" when no audit session is active
    if session_id == "unset" || session_id.is_empty() {
        return Err(psh_core::PshError::Other(
            "session id is 'unset' or empty — ensure $XDG_SESSION_ID is set or a logind session is active".into(),
        ));
    }

    tracing::debug!("detected session id: {session_id}");

    let mut details = HashMap::new();
    details.insert(
        "session-id".to_string(),
        Value::new(session_id).try_into().map_err(|e| {
            psh_core::PshError::Other(format!("failed to create session-id value: {e}"))
        })?,
    );

    Ok(("unix-session".to_string(), details))
}

// ---------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------

/// Register this agent with the polkit authority for the current session.
pub async fn register(conn: &Connection, subject: &Subject) -> Result<()> {
    let proxy = AuthorityProxy::new(conn).await?;
    let locale = std::env::var("LANG").unwrap_or_else(|_| "en_US.UTF-8".to_string());
    proxy
        .register_authentication_agent(subject, &locale, AGENT_OBJECT_PATH)
        .await?;
    tracing::info!("registered with polkit authority");
    Ok(())
}

/// Unregister this agent from the polkit authority.
pub async fn unregister(conn: &Connection, subject: &Subject) -> Result<()> {
    let proxy = AuthorityProxy::new(conn).await?;
    proxy
        .unregister_authentication_agent(subject, AGENT_OBJECT_PATH)
        .await?;
    tracing::info!("unregistered from polkit authority");
    Ok(())
}

// ---------------------------------------------------------------------------
// Authentication response
// ---------------------------------------------------------------------------

/// Notify polkitd that authentication succeeded for `identity` on `cookie`.
///
/// The uid passed to `authentication_agent_response2` must be the real uid of
/// the calling process (this agent), not the target user's uid.
pub async fn respond(conn: &Connection, cookie: &str, identity: &Identity) -> Result<()> {
    let caller_uid = unsafe { libc::getuid() };
    let proxy = AuthorityProxy::new(conn).await?;
    proxy
        .authentication_agent_response2(caller_uid, cookie, identity)
        .await?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Identity helpers
// ---------------------------------------------------------------------------

/// Extract the first `unix-user` uid and its raw identity from the identity list.
pub fn extract_uid(identities: &[Identity]) -> Option<(u32, Identity)> {
    for (kind, details) in identities {
        if kind == "unix-user"
            && let Some(val) = details.get("uid")
            && let Ok(uid) = <u32>::try_from(val)
        {
            return Some((uid, (kind.clone(), details.clone())));
        }
    }
    None
}

/// Look up a username by uid via NSS (`getpwuid_r`).
///
/// Unlike manual `/etc/passwd` parsing, this correctly resolves usernames from
/// LDAP, SSSD, NIS, and other NSS-configured sources.
pub fn uid_to_username(uid: u32) -> Option<String> {
    let mut buf = [0u8; 4096];
    let mut pwd: libc::passwd = unsafe { std::mem::zeroed() };
    let mut result: *mut libc::passwd = std::ptr::null_mut();

    let ret = unsafe {
        libc::getpwuid_r(
            uid,
            &mut pwd,
            buf.as_mut_ptr() as *mut libc::c_char,
            buf.len(),
            &mut result,
        )
    };

    if ret != 0 || result.is_null() {
        return None;
    }

    let name = unsafe { std::ffi::CStr::from_ptr(pwd.pw_name) };
    name.to_str().ok().map(|s| s.to_string())
}

// ---------------------------------------------------------------------------
// Password verification via polkit-agent-helper-1
// ---------------------------------------------------------------------------

/// Authenticate `username` with `password` by spawning `polkit-agent-helper-1`.
///
/// The helper reads the password from stdin, authenticates via PAM, and exits
/// with status 0 on success. This is the standard approach used by lightweight
/// polkit agents.
pub async fn authenticate(username: &str, password: &SecretString) -> Result<bool> {
    let mut child = tokio::process::Command::new(POLKIT_HELPER)
        .arg(username)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| psh_core::PshError::Other(format!("failed to spawn {POLKIT_HELPER}: {e}")))?;

    if let Some(mut stdin) = child.stdin.take() {
        use tokio::io::AsyncWriteExt;
        let secret = password.expose_secret();
        // polkit-agent-helper-1 expects the password followed by a newline
        let write_result = async {
            stdin.write_all(secret.as_bytes()).await?;
            stdin.write_all(b"\n").await?;
            drop(stdin);
            Ok::<(), std::io::Error>(())
        }
        .await;
        if let Err(e) = write_result {
            let _ = child.kill().await;
            return Err(psh_core::PshError::Other(format!(
                "failed to write to polkit helper stdin: {e}"
            )));
        }
    }

    // Timeout the helper to avoid blocking indefinitely if PAM hangs.
    match tokio::time::timeout(std::time::Duration::from_secs(120), child.wait()).await {
        Ok(Ok(status)) => Ok(status.success()),
        Ok(Err(e)) => {
            let _ = child.kill().await;
            Err(e.into())
        }
        Err(_timeout) => {
            tracing::warn!("polkit helper timed out after 120s, killing");
            let _ = child.kill().await;
            Err(psh_core::PshError::Other(
                "polkit helper timed out".to_string(),
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use zbus::zvariant::Value;

    /// Helper to build an `Identity` tuple for tests.
    fn make_identity(kind: &str, uid: u32) -> Identity {
        let mut details = HashMap::new();
        let val: OwnedValue = Value::new(uid).try_into().unwrap();
        details.insert("uid".to_string(), val);
        (kind.to_string(), details)
    }

    #[test]
    fn extract_uid_finds_unix_user() {
        let identities = vec![make_identity("unix-user", 1000)];
        let (uid, identity) = extract_uid(&identities).unwrap();
        assert_eq!(uid, 1000);
        assert_eq!(identity.0, "unix-user");
    }

    #[test]
    fn extract_uid_skips_non_unix_user() {
        let identities = vec![make_identity("unix-group", 100)];
        assert!(extract_uid(&identities).is_none());
    }

    #[test]
    fn extract_uid_returns_first_unix_user() {
        let identities = vec![
            make_identity("unix-group", 100),
            make_identity("unix-user", 1000),
            make_identity("unix-user", 0),
        ];
        let (uid, _) = extract_uid(&identities).unwrap();
        assert_eq!(uid, 1000);
    }

    #[test]
    fn extract_uid_empty_list() {
        let identities: Vec<Identity> = vec![];
        assert!(extract_uid(&identities).is_none());
    }

    #[test]
    fn uid_to_username_resolves_root() {
        // uid 0 (root) should always be resolvable on any Linux system.
        let name = uid_to_username(0);
        assert_eq!(name.as_deref(), Some("root"));
    }

    #[test]
    fn uid_to_username_returns_none_for_nonexistent() {
        // A very high uid that almost certainly doesn't exist.
        let name = uid_to_username(u32::MAX - 1);
        assert!(name.is_none());
    }

    // SAFETY: These tests use set_var/remove_var which are unsafe in edition 2024
    // because they are not thread-safe. We run these tests with --test-threads=1.

    #[test]
    fn detect_session_subject_uses_env_var() {
        unsafe { std::env::set_var("XDG_SESSION_ID", "test-42") };
        let subject = detect_session_subject().unwrap();
        assert_eq!(subject.0, "unix-session");
        let session_id_val = subject.1.get("session-id").expect("missing session-id key");
        let session_id = <&str>::try_from(session_id_val).expect("session-id not a string");
        assert_eq!(session_id, "test-42");
        unsafe { std::env::remove_var("XDG_SESSION_ID") };
    }

    #[test]
    fn detect_session_subject_rejects_unset() {
        unsafe { std::env::set_var("XDG_SESSION_ID", "unset") };
        let result = detect_session_subject();
        assert!(result.is_err());
        unsafe { std::env::remove_var("XDG_SESSION_ID") };
    }

    #[test]
    fn detect_session_subject_rejects_empty() {
        unsafe { std::env::set_var("XDG_SESSION_ID", "") };
        let result = detect_session_subject();
        assert!(result.is_err());
        unsafe { std::env::remove_var("XDG_SESSION_ID") };
    }
}
