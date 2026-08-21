//! CLI formatting and utility helpers.
//!
//! Timestamp formatting, path absolutization, and time utilities used by
//! the command handlers.

use crate::error::{Result, VarnError};
use std::path::PathBuf;

/// Format a UNIX timestamp as `YYYY-MM-DD HH:MM` (UTC).
pub fn format_timestamp(ts: i64) -> String {
    // Simple formatting without external dependencies.
    if ts < 0 {
        return "1970-01-01 00:00".to_string();
    }
    let secs = ts as u64;
    let days_since_epoch = secs / 86400;
    let time_of_day = secs % 86400;
    let hour = time_of_day / 3600;
    let minute = (time_of_day % 3600) / 60;

    // Compute date from days since 1970-01-01.
    let (year, month, day) = days_to_date(days_since_epoch);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}")
}

/// Convert days since 1970-01-01 to (year, month, day).
/// Based on the Howard Hinnant date algorithm.
pub fn days_to_date(days: u64) -> (u32, u32, u32) {
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = z - era * 146097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11]
    let d = doy - (153 * mp + 2) / 5 + 1; // [1, 31]
    let m = if mp < 10 { mp + 3 } else { mp - 9 }; // [1, 12]
    let year = if m <= 2 { y + 1 } else { y };
    (year as u32, m as u32, d as u32)
}

/// Current time as seconds since the UNIX epoch.
pub fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Resolve a possibly-relative path to an absolute one without following
/// symlinks in the final component.
pub fn absolutize(path: &PathBuf) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.clone());
    }
    let cwd = std::env::current_dir()
        .map_err(|e| VarnError::Other(format!("could not determine current directory: {e}")))?;
    Ok(cwd.join(path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_timestamp_known_value() {
        // 2026-08-19 20:14 in UTC (timestamp 1787162040)
        let ts = 1787162040;
        let formatted = format_timestamp(ts);
        assert!(formatted.starts_with("2026-08-19"));
    }

    #[test]
    fn days_to_date_epoch() {
        let (y, m, d) = days_to_date(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }

    #[test]
    fn days_to_date_known() {
        let (y, m, d) = days_to_date(20684);
        assert_eq!((y, m, d), (2026, 8, 19));
    }

    #[test]
    fn format_timestamp_negative_clamps_to_epoch() {
        let formatted = format_timestamp(-1);
        assert_eq!(formatted, "1970-01-01 00:00");
    }

    #[test]
    fn format_timestamp_zero_is_epoch() {
        let formatted = format_timestamp(0);
        assert_eq!(formatted, "1970-01-01 00:00");
    }
}
