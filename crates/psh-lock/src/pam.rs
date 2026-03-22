//! PAM authentication for psh-lock.
//!
//! Runs PAM on a dedicated thread to avoid blocking the Wayland event loop.
//! Uses a custom [`ConversationHandler`] that supplies the password collected
//! from keyboard input.

use pam_client::conv_mock::Conversation;
use pam_client::{Context, Flag};
use smithay_client_toolkit::reexports::calloop::channel::Sender;
use zeroize::Zeroize;

/// Result of a PAM authentication attempt.
#[derive(Debug)]
pub enum PamResult {
    Success,
    Failed(String),
}

/// Spawn a PAM authentication attempt on a dedicated thread.
///
/// The result is sent back via the calloop channel `sender` so the main event
/// loop can handle it without blocking.
/// Takes ownership of the password string so the caller's copy can be
/// immediately zeroized, ensuring only one plaintext copy exists at a time.
pub fn try_authenticate(mut password: String, sender: Sender<PamResult>) {
    std::thread::spawn(move || {
        let result = authenticate_pam(&password);
        password.zeroize();
        let _ = sender.send(result);
    });
}

/// Run PAM authentication with the given password.
///
/// Uses `pam_client::conv_mock::Conversation` which stores the password and
/// supplies it when PAM asks for it via the conversation function.
fn authenticate_pam(password: &str) -> PamResult {
    let conversation = Conversation::with_credentials("", password);
    let mut context = match Context::new("psh-lock", None, conversation) {
        Ok(ctx) => ctx,
        Err(e) => {
            tracing::error!("PAM context creation failed: {e}");
            return PamResult::Failed("System error".into());
        }
    };

    match context.authenticate(Flag::NONE) {
        Ok(()) => {
            tracing::info!("PAM authentication succeeded");
            // Also check account validity.
            if let Err(e) = context.acct_mgmt(Flag::NONE) {
                tracing::warn!("PAM account check failed: {e}");
                return PamResult::Failed("Account error".into());
            }
            PamResult::Success
        }
        Err(e) => {
            tracing::warn!("PAM authentication failed: {e}");
            PamResult::Failed("Authentication failed".into())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pam_result_debug() {
        let success = PamResult::Success;
        let failed = PamResult::Failed("bad password".into());
        assert!(format!("{success:?}").contains("Success"));
        assert!(format!("{failed:?}").contains("bad password"));
    }
}
