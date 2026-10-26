//! Daily recording windows (local time).

use chrono::{Local, NaiveTime};

use crate::config::ScheduleConfig;

/// Whether recording is allowed right now for the configured schedule.
pub fn recording_allowed(schedule: &ScheduleConfig) -> bool {
    if !schedule.enabled {
        return true;
    }
    let now = Local::now().time();
    let start = minutes_to_time(schedule.start_minutes);
    let end = minutes_to_time(schedule.end_minutes);
    within_window(now, start, end)
}

fn minutes_to_time(minutes: u32) -> NaiveTime {
    let minutes = minutes.min(24 * 60 - 1);
    NaiveTime::from_hms_opt(minutes / 60, minutes % 60, 0).unwrap_or(NaiveTime::MIN)
}

fn within_window(now: NaiveTime, start: NaiveTime, end: NaiveTime) -> bool {
    if start == end {
        // Same start/end means “all day” when the schedule is enabled.
        return true;
    }
    if start < end {
        now >= start && now < end
    } else {
        // Overnight window, e.g. 22:00–06:00.
        now >= start || now < end
    }
}

pub fn format_hhmm(minutes: u32) -> String {
    let minutes = minutes.min(24 * 60 - 1);
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

pub fn parse_hhmm(text: &str) -> Option<u32> {
    let text = text.trim();
    let (h, m) = text.split_once(':')?;
    let hours: u32 = h.parse().ok()?;
    let mins: u32 = m.parse().ok()?;
    if hours > 23 || mins > 59 {
        return None;
    }
    Some(hours * 60 + mins)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn daytime_and_overnight_windows() {
        let noon = NaiveTime::from_hms_opt(12, 0, 0).unwrap();
        let start = NaiveTime::from_hms_opt(9, 0, 0).unwrap();
        let end = NaiveTime::from_hms_opt(18, 0, 0).unwrap();
        assert!(within_window(noon, start, end));
        assert!(!within_window(
            NaiveTime::from_hms_opt(8, 0, 0).unwrap(),
            start,
            end
        ));

        let night_start = NaiveTime::from_hms_opt(22, 0, 0).unwrap();
        let night_end = NaiveTime::from_hms_opt(6, 0, 0).unwrap();
        assert!(within_window(
            NaiveTime::from_hms_opt(23, 0, 0).unwrap(),
            night_start,
            night_end
        ));
        assert!(within_window(
            NaiveTime::from_hms_opt(3, 0, 0).unwrap(),
            night_start,
            night_end
        ));
        assert!(!within_window(noon, night_start, night_end));
    }

    #[test]
    fn parses_hhmm() {
        assert_eq!(parse_hhmm("09:30"), Some(9 * 60 + 30));
        assert_eq!(parse_hhmm("24:00"), None);
        assert_eq!(format_hhmm(9 * 60 + 5), "09:05");
    }
}
