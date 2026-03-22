//! Claude module — displays Claude.ai usage quota in the bar.
//!
//! Polls the claude.ai web API using a session cookie to fetch quota
//! percentages (session and weekly remaining). Follows the same async
//! pattern as the network module: tokio backend → async_channel → GTK.

use gtk4::glib;
use gtk4::prelude::*;

use super::{BarModule, ModuleContext};

/// Displays Claude.ai usage quota.
///
/// Shows remaining quota as a percentage with optional session/weekly
/// breakdown. Requires a session key via config or `CLAUDE_SESSION_KEY`
/// env var.
pub struct ClaudeModule;

/// Parsed usage state from the claude.ai API.
#[derive(Debug, Clone)]
pub(crate) struct ClaudeUsage {
    /// Session-level remaining percentage (0.0–100.0).
    pub session_pct: f64,
    /// Weekly remaining percentage (0.0–100.0).
    pub weekly_pct: f64,
    /// Per-model breakdown (model name, remaining %).
    pub models: Vec<(String, f64)>,
    /// When the session quota resets (human-readable).
    pub session_reset: Option<String>,
    /// When the weekly quota resets (human-readable).
    pub weekly_reset: Option<String>,
}

/// Display format for the module.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum DisplayFormat {
    /// Show session remaining percentage only.
    Percent,
    /// Show both session and weekly percentages.
    Both,
}

/// Result sent from the backend to the GTK thread.
type UsageResult = Result<ClaudeUsage, String>;

/// CSS class names used to indicate usage level.
const LEVEL_CLASSES: &[&str] = &["ok", "low", "critical", "error"];

/// Base URL for the claude.ai API.
const CLAUDE_API_BASE: &str = "https://claude.ai";

/// User-Agent header mimicking a browser.
const USER_AGENT: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:128.0) Gecko/20100101 Firefox/128.0";

impl BarModule for ClaudeModule {
    fn name(&self) -> &'static str {
        "claude"
    }

    fn widget(&self, ctx: &ModuleContext) -> gtk4::Widget {
        let label = gtk4::Label::new(Some("CLAUDE --"));
        label.add_css_class("psh-bar-claude");
        label.set_tooltip_text(Some("Claude.ai usage"));

        // Resolve session key: config first, then env var
        let session_key = ctx
            .config
            .claude_session_key
            .clone()
            .or_else(|| std::env::var("CLAUDE_SESSION_KEY").ok());

        let display_format = parse_display_format(ctx.config.claude_display.as_deref());
        let poll_interval = ctx.config.claude_poll_interval.unwrap_or(120);

        if let Some(key) = session_key {
            let (tx, rx) = async_channel::bounded::<UsageResult>(4);

            ctx.rt.spawn(async move {
                run_claude_backend(tx, key, poll_interval).await;
            });

            let label_clone = label.clone();
            glib::spawn_future_local(async move {
                while let Ok(result) = rx.recv().await {
                    update_label(&label_clone, &result, &display_format);
                }
            });
        } else {
            label.set_text("CLAUDE: no key");
            label.add_css_class("error");
        }

        label.upcast()
    }
}

/// Poll the claude.ai API on a regular interval with exponential backoff.
async fn run_claude_backend(
    tx: async_channel::Sender<UsageResult>,
    session_key: String,
    base_interval: u64,
) {
    let client = match reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .build()
    {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.send(Err(format!("HTTP client init failed: {e}"))).await;
            return;
        }
    };

    let mut org_id: Option<String> = None;
    let mut consecutive_failures: u32 = 0;

    loop {
        let result = fetch_usage(&client, &session_key, &mut org_id).await;

        let is_err = result.is_err();
        let is_auth_err = matches!(&result, Err(e) if e.contains("auth"));

        if tx.send(result).await.is_err() {
            return; // GTK side dropped
        }

        // On auth failure, stop polling — the session key is invalid
        if is_auth_err {
            tracing::warn!("claude module: session key expired or invalid, stopping");
            return;
        }

        if is_err {
            consecutive_failures = consecutive_failures.saturating_add(1);
        } else {
            consecutive_failures = 0;
        }

        // Exponential backoff: 1x, 2x, 4x, max 8x base interval
        let multiplier = match consecutive_failures {
            0..=1 => 1u64,
            2..=3 => 2,
            4..=5 => 4,
            _ => 8,
        };
        let sleep_secs = base_interval.saturating_mul(multiplier);
        tokio::time::sleep(std::time::Duration::from_secs(sleep_secs)).await;
    }
}

/// Fetch usage data from claude.ai.
///
/// Resolves the org ID on first call and caches it in `org_id`.
async fn fetch_usage(
    client: &reqwest::Client,
    session_key: &str,
    org_id: &mut Option<String>,
) -> Result<ClaudeUsage, String> {
    // Step 1: resolve org ID if not cached
    if org_id.is_none() {
        let id = fetch_org_id(client, session_key).await?;
        *org_id = Some(id);
    }

    let oid = org_id.as_ref().unwrap();

    // Step 2: fetch usage
    let url = format!("{CLAUDE_API_BASE}/api/organizations/{oid}/usage");
    let resp = client
        .get(&url)
        .header("Cookie", format!("sessionKey={session_key}"))
        .header("anthropic-client-platform", "web_claude_ai")
        .send()
        .await
        .map_err(|e| format!("usage request failed: {e}"))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        // Clear cached org ID so re-auth can be attempted
        *org_id = None;
        return Err("auth: session key expired or invalid".into());
    }

    let resp = resp
        .error_for_status()
        .map_err(|e| format!("usage API error: {e}"))?;

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| format!("usage parse error: {e}"))?;

    parse_usage_response(&body)
}

/// Fetch the organization UUID from claude.ai.
async fn fetch_org_id(
    client: &reqwest::Client,
    session_key: &str,
) -> Result<String, String> {
    let url = format!("{CLAUDE_API_BASE}/api/organizations");
    let resp = client
        .get(&url)
        .header("Cookie", format!("sessionKey={session_key}"))
        .header("anthropic-client-platform", "web_claude_ai")
        .send()
        .await
        .map_err(|e| format!("org request failed: {e}"))?;

    let status = resp.status();
    if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
        return Err("auth: session key expired or invalid".into());
    }

    let resp = resp
        .error_for_status()
        .map_err(|e| format!("org API error: {e}"))?;

    let orgs: Vec<serde_json::Value> = resp
        .json()
        .await
        .map_err(|e| format!("org parse error: {e}"))?;

    orgs.first()
        .and_then(|o| o.get("uuid"))
        .and_then(|u| u.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "no organization found in response".into())
}

/// Parse the usage API response JSON into a `ClaudeUsage`.
///
/// The API returns utilization (% used) under `five_hour` and `seven_day`
/// objects, each with `utilization` and `resets_at` fields. Per-model
/// breakdowns appear as `seven_day_{model}` keys.
pub(crate) fn parse_usage_response(body: &serde_json::Value) -> Result<ClaudeUsage, String> {
    let session_pct = body
        .get("five_hour")
        .and_then(|v| v.get("utilization"))
        .and_then(|v| v.as_f64())
        .map(|u| 100.0 - u)
        .unwrap_or(0.0);

    let weekly_pct = body
        .get("seven_day")
        .and_then(|v| v.get("utilization"))
        .and_then(|v| v.as_f64())
        .map(|u| 100.0 - u)
        .unwrap_or(0.0);

    let session_reset = body
        .get("five_hour")
        .and_then(|v| v.get("resets_at"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let weekly_reset = body
        .get("seven_day")
        .and_then(|v| v.get("resets_at"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    // Per-model breakdowns appear as seven_day_{model} keys
    let mut models = Vec::new();
    if let Some(obj) = body.as_object() {
        for (key, val) in obj {
            if let Some(model) = key.strip_prefix("seven_day_") {
                if let Some(util) = val.get("utilization").and_then(|v| v.as_f64()) {
                    models.push((model.to_string(), 100.0 - util));
                }
            }
        }
    }

    Ok(ClaudeUsage {
        session_pct,
        weekly_pct,
        models,
        session_reset,
        weekly_reset,
    })
}

/// Parse display format from config string.
pub(crate) fn parse_display_format(s: Option<&str>) -> DisplayFormat {
    match s {
        Some("percent") => DisplayFormat::Percent,
        _ => DisplayFormat::Both,
    }
}

/// Format usage for display in the bar label.
pub(crate) fn format_usage(usage: &ClaudeUsage, format: &DisplayFormat) -> String {
    match format {
        DisplayFormat::Percent => format!("CLAUDE {:.0}%", usage.session_pct),
        DisplayFormat::Both => {
            format!("CLAUDE S:{:.0}% W:{:.0}%", usage.session_pct, usage.weekly_pct)
        }
    }
}

/// Format an error for display in the bar label.
pub(crate) fn format_error(err: &str) -> String {
    if err.contains("auth") {
        "CLAUDE: auth expired".into()
    } else {
        "CLAUDE: error".into()
    }
}

/// Build a tooltip string with full usage details.
pub(crate) fn format_tooltip(usage: &ClaudeUsage) -> String {
    let mut lines = vec![
        format!("Session: {:.1}% remaining", usage.session_pct),
        format!("Weekly: {:.1}% remaining", usage.weekly_pct),
    ];

    if let Some(ref reset) = usage.session_reset {
        lines.push(format!("Session resets: {reset}"));
    }
    if let Some(ref reset) = usage.weekly_reset {
        lines.push(format!("Weekly resets: {reset}"));
    }

    for (model, pct) in &usage.models {
        lines.push(format!("{model}: {pct:.1}%"));
    }

    lines.join("\n")
}

/// Determine the CSS class for the current usage level.
pub(crate) fn css_class_for_usage(pct: f64) -> &'static str {
    if pct > 30.0 {
        "ok"
    } else if pct > 10.0 {
        "low"
    } else {
        "critical"
    }
}

/// Update the GTK label with the latest usage result.
fn update_label(label: &gtk4::Label, result: &UsageResult, format: &DisplayFormat) {
    // Clear previous level classes
    for cls in LEVEL_CLASSES {
        label.remove_css_class(cls);
    }

    match result {
        Ok(usage) => {
            label.set_text(&format_usage(usage, format));
            label.set_tooltip_text(Some(&format_tooltip(usage)));
            label.add_css_class(css_class_for_usage(usage.session_pct));
        }
        Err(err) => {
            label.set_text(&format_error(err));
            label.set_tooltip_text(Some(err));
            label.add_css_class("error");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_display_format() {
        assert_eq!(parse_display_format(None), DisplayFormat::Both);
        assert_eq!(parse_display_format(Some("percent")), DisplayFormat::Percent);
        assert_eq!(parse_display_format(Some("both")), DisplayFormat::Both);
        assert_eq!(parse_display_format(Some("invalid")), DisplayFormat::Both);
        assert_eq!(parse_display_format(Some("")), DisplayFormat::Both);
    }

    #[test]
    fn test_format_usage_percent() {
        let usage = ClaudeUsage {
            session_pct: 73.5,
            weekly_pct: 85.2,
            models: vec![],
            session_reset: None,
            weekly_reset: None,
        };
        assert_eq!(format_usage(&usage, &DisplayFormat::Percent), "CLAUDE 74%");
    }

    #[test]
    fn test_format_usage_both() {
        let usage = ClaudeUsage {
            session_pct: 73.5,
            weekly_pct: 85.2,
            models: vec![],
            session_reset: None,
            weekly_reset: None,
        };
        assert_eq!(
            format_usage(&usage, &DisplayFormat::Both),
            "CLAUDE S:74% W:85%"
        );
    }

    #[test]
    fn test_format_usage_zero() {
        let usage = ClaudeUsage {
            session_pct: 0.0,
            weekly_pct: 0.0,
            models: vec![],
            session_reset: None,
            weekly_reset: None,
        };
        assert_eq!(format_usage(&usage, &DisplayFormat::Percent), "CLAUDE 0%");
    }

    #[test]
    fn test_format_error_auth() {
        assert_eq!(format_error("auth: session key expired"), "CLAUDE: auth expired");
    }

    #[test]
    fn test_format_error_generic() {
        assert_eq!(format_error("usage request failed: timeout"), "CLAUDE: error");
    }

    #[test]
    fn test_css_class_for_usage() {
        assert_eq!(css_class_for_usage(100.0), "ok");
        assert_eq!(css_class_for_usage(50.0), "ok");
        assert_eq!(css_class_for_usage(31.0), "ok");
        assert_eq!(css_class_for_usage(30.0), "low");
        assert_eq!(css_class_for_usage(15.0), "low");
        assert_eq!(css_class_for_usage(10.0), "critical");
        assert_eq!(css_class_for_usage(5.0), "critical");
        assert_eq!(css_class_for_usage(0.0), "critical");
    }

    #[test]
    fn test_parse_usage_response() {
        let json = serde_json::json!({
            "five_hour": {
                "utilization": 4.0,
                "resets_at": "2026-03-23T01:00:00Z"
            },
            "seven_day": {
                "utilization": 15.0,
                "resets_at": "2026-03-27T00:00:00Z"
            },
            "seven_day_opus": {
                "utilization": 40.0,
                "resets_at": "2026-03-27T00:00:00Z"
            },
            "seven_day_sonnet": {
                "utilization": 1.0,
                "resets_at": "2026-03-27T00:00:00Z"
            }
        });

        let usage = parse_usage_response(&json).unwrap();
        assert!((usage.session_pct - 96.0).abs() < f64::EPSILON);
        assert!((usage.weekly_pct - 85.0).abs() < f64::EPSILON);
        assert_eq!(usage.session_reset.as_deref(), Some("2026-03-23T01:00:00Z"));
        assert_eq!(usage.weekly_reset.as_deref(), Some("2026-03-27T00:00:00Z"));
        assert_eq!(usage.models.len(), 2);
    }

    #[test]
    fn test_parse_usage_response_minimal() {
        let json = serde_json::json!({});
        let usage = parse_usage_response(&json).unwrap();
        assert!((usage.session_pct - 0.0).abs() < f64::EPSILON);
        assert!((usage.weekly_pct - 0.0).abs() < f64::EPSILON);
        assert!(usage.models.is_empty());
    }

    #[test]
    fn test_format_tooltip() {
        let usage = ClaudeUsage {
            session_pct: 73.5,
            weekly_pct: 85.2,
            models: vec![("opus".into(), 60.0), ("sonnet".into(), 90.0)],
            session_reset: Some("2026-03-21T18:00:00Z".into()),
            weekly_reset: None,
        };
        let tooltip = format_tooltip(&usage);
        assert!(tooltip.contains("Session: 73.5% remaining"));
        assert!(tooltip.contains("Weekly: 85.2% remaining"));
        assert!(tooltip.contains("Session resets: 2026-03-21T18:00:00Z"));
        assert!(tooltip.contains("opus: 60.0%"));
        assert!(tooltip.contains("sonnet: 90.0%"));
    }
}
