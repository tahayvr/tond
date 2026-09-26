// alerts/quiet.rs — quiet hours
//
// A daily window (e.g. 22:00–07:00 Europe/Berlin) during which notifications
// wait instead of being sent. The window may span midnight. Critical alerts
// bypass it unless `critical_bypass` is off. Deliveries held back are sent
// when the window ends — unless the alert resolved in the meantime, in which
// case neither message is sent (see deliver.rs).
//
// Each channel follows the global setting (settings key `alert_quiet_hours`),
// has none, or has its own window.

use chrono::{Duration, NaiveTime, TimeZone};
use sqlx::SqlitePool;
use tond_common::{ChannelQuietHours, QuietHours};

/// Settings key of the global quiet hours (JSON `QuietHours`).
pub const SETTING_KEY: &str = "alert_quiet_hours";

pub fn parse_hhmm(s: &str) -> Option<NaiveTime> {
    NaiveTime::parse_from_str(s.trim(), "%H:%M").ok()
}

/// Check a window before it is saved; the error is shown to the user.
pub fn validate(q: &QuietHours) -> Result<(), String> {
    if parse_hhmm(&q.start).is_none() || parse_hhmm(&q.end).is_none() {
        return Err("Quiet hours need a start and end time like 22:00.".to_string());
    }
    if q.start.trim() == q.end.trim() {
        return Err("Quiet hours need different start and end times.".to_string());
    }
    if q.timezone.parse::<chrono_tz::Tz>().is_err() {
        return Err(format!(
            "Unknown time zone '{}'. Use a name like Europe/Berlin.",
            q.timezone
        ));
    }
    Ok(())
}

/// The window that applies to a channel, if any.
pub fn effective<'a>(
    channel: &'a ChannelQuietHours,
    global: &'a QuietHours,
) -> Option<&'a QuietHours> {
    let q = match channel {
        ChannelQuietHours::Global => global,
        ChannelQuietHours::Off => return None,
        ChannelQuietHours::Custom(q) => q,
    };
    q.enabled.then_some(q)
}

/// When `now` (unix seconds) falls inside the window, the unix time it ends.
pub fn quiet_until(q: &QuietHours, severity: &str, now: i64) -> Option<i64> {
    if !q.enabled || (severity == "critical" && q.critical_bypass) {
        return None;
    }
    let (start, end) = (parse_hhmm(&q.start)?, parse_hhmm(&q.end)?);
    if start == end {
        return None;
    }
    let tz: chrono_tz::Tz = q.timezone.parse().unwrap_or(chrono_tz::UTC);
    let local = tz.timestamp_opt(now, 0).single()?;
    let t = local.time();
    let today = local.date_naive();
    let end_date = if start < end {
        if t < start || t >= end {
            return None;
        }
        today
    } else if t >= start {
        today + Duration::days(1)
    } else if t < end {
        today
    } else {
        return None;
    };
    let end_local = end_date.and_time(end);
    // A DST gap can swallow the end time: fall back to an hour later.
    let end_ts = tz
        .from_local_datetime(&end_local)
        .earliest()
        .or_else(|| {
            tz.from_local_datetime(&(end_local + Duration::hours(1)))
                .earliest()
        })?
        .timestamp();
    (end_ts > now).then_some(end_ts)
}

/// The global quiet hours (defaults: off).
pub async fn load_global(db: &SqlitePool) -> QuietHours {
    let raw: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key = ?")
        .bind(SETTING_KEY)
        .fetch_optional(db)
        .await
        .ok()
        .flatten();
    raw.and_then(|r| serde_json::from_str(&r).ok())
        .unwrap_or_default()
}

pub async fn save_global(db: &SqlitePool, q: &QuietHours) -> sqlx::Result<()> {
    let json = serde_json::to_string(q).unwrap_or_default();
    sqlx::query(
        "INSERT INTO settings (key, value, updated_at) VALUES (?, ?, datetime('now'))
         ON CONFLICT(key) DO UPDATE SET value = excluded.value, updated_at = excluded.updated_at",
    )
    .bind(SETTING_KEY)
    .bind(json)
    .execute(db)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(start: &str, end: &str, tz: &str) -> QuietHours {
        QuietHours {
            enabled: true,
            start: start.to_string(),
            end: end.to_string(),
            timezone: tz.to_string(),
            critical_bypass: true,
        }
    }

    fn ts(s: &str) -> i64 {
        chrono::DateTime::parse_from_rfc3339(s)
            .map(|t| t.timestamp())
            .unwrap_or_else(|err| panic!("{s}: {err}"))
    }

    #[test]
    fn a_window_spanning_midnight_ends_the_next_morning() {
        let q = window("22:00", "07:00", "UTC");
        assert_eq!(
            quiet_until(&q, "warning", ts("2026-03-10T23:30:00Z")),
            Some(ts("2026-03-11T07:00:00Z"))
        );
        assert_eq!(
            quiet_until(&q, "warning", ts("2026-03-11T06:59:00Z")),
            Some(ts("2026-03-11T07:00:00Z"))
        );
        assert_eq!(quiet_until(&q, "warning", ts("2026-03-11T07:00:00Z")), None);
        assert_eq!(quiet_until(&q, "warning", ts("2026-03-11T12:00:00Z")), None);
    }

    #[test]
    fn a_daytime_window_and_time_zones() {
        // 12:00–13:00 in Berlin (UTC+1 in winter) is 11:00–12:00 UTC.
        let q = window("12:00", "13:00", "Europe/Berlin");
        assert_eq!(
            quiet_until(&q, "info", ts("2026-01-15T11:30:00Z")),
            Some(ts("2026-01-15T12:00:00Z"))
        );
        assert_eq!(quiet_until(&q, "info", ts("2026-01-15T12:30:00Z")), None);
    }

    #[test]
    fn critical_bypasses_unless_disabled_and_disabled_windows_do_nothing() {
        let mut q = window("00:00", "23:59", "UTC");
        let now = ts("2026-01-15T11:30:00Z");
        assert_eq!(quiet_until(&q, "critical", now), None);
        q.critical_bypass = false;
        assert!(quiet_until(&q, "critical", now).is_some());
        q.enabled = false;
        assert_eq!(quiet_until(&q, "warning", now), None);
    }

    #[test]
    fn channels_follow_global_off_or_custom() {
        let global = window("22:00", "07:00", "UTC");
        assert_eq!(
            effective(&ChannelQuietHours::Global, &global),
            Some(&global)
        );
        assert_eq!(effective(&ChannelQuietHours::Off, &global), None);
        let own = window("01:00", "02:00", "UTC");
        assert_eq!(
            effective(&ChannelQuietHours::Custom(own.clone()), &global),
            Some(&own)
        );
        let off_global = QuietHours::default();
        assert_eq!(effective(&ChannelQuietHours::Global, &off_global), None);
    }

    #[test]
    fn validation_messages() {
        assert!(validate(&window("22:00", "07:00", "Europe/Berlin")).is_ok());
        assert!(validate(&window("25:00", "07:00", "UTC")).is_err());
        assert!(validate(&window("07:00", "07:00", "UTC")).is_err());
        assert!(
            validate(&window("22:00", "07:00", "Mars/Olympus"))
                .err()
                .unwrap_or_default()
                .contains("Unknown time zone")
        );
    }
}
