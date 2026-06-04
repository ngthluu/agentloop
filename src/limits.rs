use std::path::Path;
use std::time::Duration;

/// A provider usage/rate limit detected in agent output.
#[derive(Debug, Clone, PartialEq)]
pub struct UsageLimit {
    /// Unix epoch (seconds) when the limit resets, when the output included one.
    pub reset_epoch: Option<i64>,
}

/// Scan agent output for a usage/rate-limit message. Patterns are kept narrow
/// (provider phrasings, not the words "rate limit" alone) so ordinary failures
/// in agents that happen to discuss rate limiting are not misdetected.
/// Known shapes:
///   claude: "Claude AI usage limit reached|1750118400"
///   claude: "You've reached your usage limit"
///   codex:  "You've hit your usage limit" / "Rate limit reached"
///   API:    "rate_limit_error"
pub fn detect_usage_limit(text: &str) -> Option<UsageLimit> {
    let lower = text.to_lowercase();
    // Known tradeoff: "rate limit reached" can also appear in app-level logs
    // (e.g. "database rate limit reached"); accepted — the cost is one wait.
    const PATTERNS: [&str; 5] = [
        "usage limit reached",
        "reached your usage limit",
        "hit your usage limit",
        "rate limit reached",
        "rate_limit_error",
    ];
    if !PATTERNS.iter().any(|p| lower.contains(p)) {
        return None;
    }
    // claude appends the reset epoch as "...usage limit reached|<epoch>".
    const EPOCH_MARK: &str = "usage limit reached|";
    let reset_epoch = lower.find(EPOCH_MARK).and_then(|i| {
        let digits: String = lower[i + EPOCH_MARK.len()..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();
        digits.parse::<i64>().ok().filter(|e| *e > 1_000_000_000)
    });
    Some(UsageLimit { reset_epoch })
}

/// How long to wait before auto-continuing: until the reset epoch plus a slack
/// margin when known, else a fallback window. Capped at 6h. The slack and
/// fallback are env-overridable (AGENTLOOP_LIMIT_SLACK_SECS,
/// AGENTLOOP_LIMIT_FALLBACK_SECS) so tests can shrink them.
pub fn wait_duration(limit: &UsageLimit, now_epoch: i64) -> Duration {
    let slack = env_secs("AGENTLOOP_LIMIT_SLACK_SECS", 60);
    let fallback = env_secs("AGENTLOOP_LIMIT_FALLBACK_SECS", 900);
    let secs = match limit.reset_epoch {
        Some(reset) => {
            let remaining = (reset - now_epoch).max(0) as u64;
            remaining.saturating_add(slack)
        }
        None => fallback,
    };
    Duration::from_secs(secs.min(6 * 3600))
}

fn env_secs(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// Last `max_bytes` of `path` as lossy UTF-8; "" when missing/unreadable.
pub fn log_tail(path: &Path, max_bytes: u64) -> String {
    log_tail_from(path, 0, max_bytes)
}

/// Like [`log_tail`], but never reads bytes before `start_offset` — used to scan
/// only the latest attempt's output when a log accumulates across retries.
pub fn log_tail_from(path: &Path, start_offset: u64, max_bytes: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let start = len.saturating_sub(max_bytes).max(start_offset.min(len));
    if f.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut buf = Vec::new();
    if f.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    String::from_utf8_lossy(&buf).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_claude_limit_with_reset_epoch() {
        let l = detect_usage_limit("blah\nClaude AI usage limit reached|1750118400\n").unwrap();
        assert_eq!(l.reset_epoch, Some(1750118400));
    }

    #[test]
    fn detects_limits_without_epoch() {
        for text in [
            "You've reached your usage limit",
            "you've hit your usage limit, try again later",
            "Rate limit reached for requests",
            r#"{"type":"error","error":{"type":"rate_limit_error"}}"#,
        ] {
            let l = detect_usage_limit(text).unwrap_or_else(|| panic!("missed: {text}"));
            assert_eq!(l.reset_epoch, None);
        }
    }

    #[test]
    fn ordinary_failures_are_not_limits() {
        for text in [
            "compile error: expected `;`",
            "tests failed: 3 passed; 1 failed",
            "we should rate limit the login endpoint",
            "",
        ] {
            assert!(detect_usage_limit(text).is_none(), "false positive: {text}");
        }
    }

    #[test]
    fn wait_until_reset_plus_slack_capped_at_six_hours() {
        let l = UsageLimit {
            reset_epoch: Some(1_700_000_300),
        };
        assert_eq!(
            wait_duration(&l, 1_700_000_000),
            Duration::from_secs(300 + 60)
        );
        // Past reset -> just the slack.
        assert_eq!(wait_duration(&l, 1_800_000_000), Duration::from_secs(60));
        // No epoch -> fallback window.
        assert_eq!(
            wait_duration(&UsageLimit { reset_epoch: None }, 0),
            Duration::from_secs(900)
        );
        // Far-future reset is capped.
        let far = UsageLimit {
            reset_epoch: Some(2_000_000_000),
        };
        assert_eq!(
            wait_duration(&far, 1_700_000_000),
            Duration::from_secs(6 * 3600)
        );
    }

    #[test]
    fn log_tail_from_skips_bytes_before_the_offset() {
        let dir = std::env::temp_dir().join(format!(
            "limits-tail-from-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.log");
        std::fs::write(&p, "usage limit reached|1700000000\nattempt two output").unwrap();
        let tail = log_tail_from(&p, 31, 16 * 1024);
        assert_eq!(tail, "attempt two output");
        assert!(
            detect_usage_limit(&tail).is_none(),
            "old limit text not rescanned"
        );
        // Offset beyond EOF is safe.
        assert_eq!(log_tail_from(&p, 10_000, 16), "");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_tail_reads_last_bytes_and_tolerates_missing_files() {
        let dir = std::env::temp_dir().join(format!(
            "limits-tail-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("x.log");
        assert_eq!(log_tail(&p, 16), "");
        std::fs::write(&p, "0123456789ABCDEF-tail").unwrap();
        assert_eq!(log_tail(&p, 5), "-tail");
        assert_eq!(
            log_tail(&p, 1000),
            "0123456789ABCDEF-tail",
            "window larger than the file reads the whole file"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
