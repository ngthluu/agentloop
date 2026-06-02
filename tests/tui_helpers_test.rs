use agentloop::tui::fmt_elapsed;
use std::time::Duration;

#[test]
fn formats_seconds_minutes_hours() {
    assert_eq!(fmt_elapsed(Duration::from_secs(0)), "0s");
    assert_eq!(fmt_elapsed(Duration::from_secs(7)), "7s");
    assert_eq!(fmt_elapsed(Duration::from_secs(59)), "59s");
    assert_eq!(fmt_elapsed(Duration::from_secs(60)), "1m00s");
    assert_eq!(fmt_elapsed(Duration::from_secs(192)), "3m12s");
    assert_eq!(fmt_elapsed(Duration::from_secs(3599)), "59m59s");
    assert_eq!(fmt_elapsed(Duration::from_secs(3600)), "1h00m");
    assert_eq!(fmt_elapsed(Duration::from_secs(3600 + 5 * 60)), "1h05m");
}
