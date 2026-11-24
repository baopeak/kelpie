//! When a subscription's usage limit lifts, read from the words the agent prints when it stops on one.
//!
//! Claude Code says "You've hit your 5-hour limit · resets 3:20am (Asia/Ho_Chi_Minh)", "… · resets 6pm (…)" or, for a
//! weekly limit, "… · resets Nov 26, 12pm (…)". The time is the clock of the computer it runs on, so it is read in local
//! time; the zone in brackets is not looked up. Anything that is not one of these forms, or that lies in the past or more
//! than eight days ahead, gives nothing: a timer is never set on a guess.

use std::sync::OnceLock;

use chrono::{DateTime, Datelike, Duration, Local, NaiveDate, NaiveTime, TimeZone};
use regex::Regex;

/// A reset further away than this is not a usage window (the same bound as the timers').
const MAX_AHEAD: Duration = Duration::days(8);

/// "resets [at] [Mon DD[,| at]] H[:MM][am|pm]".
fn pattern() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"(?i)\bresets\s+(?:at\s+)?(?:(?P<mon>[a-z]{3,9})\.?\s+(?P<day>\d{1,2})(?:,\s*|\s+at\s+|\s+))?(?P<h>\d{1,2})(?::(?P<m>\d{2}))?\s*(?P<ap>am|pm)?(?:[^a-z0-9:]|$)").expect("reset pattern")
    })
}

fn month(name: &str) -> Option<u32> {
    const NAMES: [&str; 12] = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"];
    let lower = name.to_ascii_lowercase();
    let i = NAMES.iter().position(|m| lower.starts_with(m))?;
    // "Nov" and "November" are a month; "Novel" is not.
    let full = ["january", "february", "march", "april", "may", "june", "july", "august", "september", "october", "november", "december"][i];
    (lower.len() == 3 || full == lower || (lower == "sept")).then_some(i as u32 + 1)
}

/// The reset time `text` names, as wall-clock ms, read in local time.
pub fn reset_at_ms(text: &str) -> Option<u64> {
    reset_at(text, Local::now()).map(|t| t.timestamp_millis()).and_then(|ms| u64::try_from(ms).ok())
}

/// The reset time `text` names, after `now` and no more than eight days ahead, in `now`'s zone.
pub fn reset_at<Tz: TimeZone>(text: &str, now: DateTime<Tz>) -> Option<DateTime<Tz>> {
    let caps = pattern().captures_iter(text).last()?;
    let hour: u32 = caps["h"].parse().ok()?;
    let minute: u32 = caps.name("m").map_or(Some(0), |m| m.as_str().parse().ok())?;
    let hour = match caps.name("ap").map(|a| a.as_str().to_ascii_lowercase()) {
        Some(ap) => {
            if !(1..=12).contains(&hour) {
                return None;
            }
            (hour % 12) + if ap == "pm" { 12 } else { 0 }
        }
        // A 24-hour clock needs its minutes: a bare "resets 3" says nothing sure.
        None if caps.name("m").is_some() => hour,
        None => return None,
    };
    let time = NaiveTime::from_hms_opt(hour, minute, 0)?;
    let zone = now.timezone();
    let local_now = now.naive_local();
    let at = match (caps.name("mon"), caps.name("day")) {
        (Some(mon), Some(day)) => {
            let (mon, day) = (month(mon.as_str())?, day.as_str().parse::<u32>().ok()?);
            // The date names no year: this year's, or next year's when this year's is long past (a reset on Jan 2 seen on Dec 30).
            let this_year = NaiveDate::from_ymd_opt(local_now.year(), mon, day)?.and_time(time);
            if this_year < local_now - Duration::days(1) {
                NaiveDate::from_ymd_opt(local_now.year() + 1, mon, day)?.and_time(time)
            } else {
                this_year
            }
        }
        (None, None) => {
            // A time alone: the next time the clock shows it. One that passed within the hour is a reset that is (or was
            // just) due, read late or rounded down ("resets 3:20am" at 3:20:31): tomorrow's would wait a day for nothing.
            let today = local_now.date().and_time(time);
            if today > local_now {
                today
            } else if local_now - today < Duration::hours(1) {
                return None;
            } else {
                today + Duration::days(1)
            }
        }
        _ => return None,
    };
    // A local time that a clock change skips or repeats is not used.
    let at = zone.from_local_datetime(&at).single()?;
    (at > now && at <= now.clone() + MAX_AHEAD).then_some(at)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    /// Ho Chi Minh City time, 2026-11-24 10:00.
    fn now() -> DateTime<FixedOffset> {
        FixedOffset::east_opt(7 * 3600).unwrap().with_ymd_and_hms(2026, 11, 24, 10, 0, 0).unwrap()
    }

    fn at(text: &str) -> Option<String> {
        reset_at(text, now()).map(|t| t.format("%Y-%m-%d %H:%M").to_string())
    }

    #[test]
    fn reads_the_times_claude_prints() {
        assert_eq!(at("You've hit your 5-hour limit · resets 3:20pm (Asia/Ho_Chi_Minh)").as_deref(), Some("2026-11-24 15:20"));
        assert_eq!(at("You've hit your 5-hour limit · resets 3:20am (Asia/Ho_Chi_Minh)").as_deref(), Some("2026-11-25 03:20"), "past today: tomorrow");
        assert_eq!(at("You've hit your 5-hour limit · resets 7:40pm").as_deref(), Some("2026-11-24 19:40"));
        assert_eq!(at("You've hit your 5-hour limit · resets 6pm (Asia/Ho_Chi_Minh)").as_deref(), Some("2026-11-24 18:00"));
        assert_eq!(at("You've hit your session limit · resets 12am (Asia/Ho_Chi_Minh)").as_deref(), Some("2026-11-25 00:00"));
        assert_eq!(at("You've hit your session limit · resets 12pm").as_deref(), Some("2026-11-24 12:00"));
        assert_eq!(at("Limit reached · resets 15:30").as_deref(), Some("2026-11-24 15:30"), "a 24-hour clock");
        assert_eq!(at("Claude usage limit reached. Your limit resets at 4pm.").as_deref(), Some("2026-11-24 16:00"));
    }

    #[test]
    fn reads_a_date() {
        assert_eq!(at("You've hit your weekly limit · resets Nov 26, 12pm (Asia/Ho_Chi_Minh)").as_deref(), Some("2026-11-26 12:00"));
        assert_eq!(at("You've hit your weekly limit · resets Nov 26, 3:20am").as_deref(), Some("2026-11-26 03:20"));
        assert_eq!(at("You've hit your weekly limit · resets Dec 1 at 9am").as_deref(), Some("2026-12-01 09:00"));
        assert_eq!(at("You've hit your weekly limit · resets November 30, 9am").as_deref(), Some("2026-11-30 09:00"));
        // The turn of the year.
        let dec30 = FixedOffset::east_opt(0).unwrap().with_ymd_and_hms(2026, 12, 30, 10, 0, 0).unwrap();
        assert_eq!(reset_at("resets Jan 2, 9am", dec30).map(|t| t.format("%Y-%m-%d").to_string()).as_deref(), Some("2027-01-02"));
    }

    #[test]
    fn refuses_what_it_cannot_be_sure_of() {
        for text in [
            "You've hit your 5-hour limit",
            "resets soon",
            "resets 3",
            "resets 13pm",
            "resets 0am",
            "resets 3:75pm",
            "resets 25:00",
            "resets Novel 26, 12pm",
            "resets Feb 30, 12pm",
            // Long past, or too far ahead: not a usage window.
            "resets Nov 20, 12pm",
            "resets Dec 20, 12pm",
            "the 3pm build resets nothing",
        ] {
            assert_eq!(at(text), None, "{text}");
        }
    }

    #[test]
    fn a_time_just_past_is_not_taken_for_tomorrow() {
        let zone = FixedOffset::east_opt(7 * 3600).unwrap();
        let at = |h, m, s| zone.with_ymd_and_hms(2026, 11, 24, h, m, s).unwrap();
        assert_eq!(reset_at("resets 3:20am", at(3, 20, 31)), None, "the reset is due: rounded down to the minute");
        assert_eq!(reset_at("resets 3:20am", at(3, 25, 0)), None);
        assert_eq!(reset_at("resets 3:20am", at(3, 20, 0)), None, "now is not ahead");
        assert_eq!(reset_at("resets 3:20am", at(4, 21, 0)).map(|t| t.format("%d %H:%M").to_string()).as_deref(), Some("25 03:20"));
    }

    #[test]
    fn the_last_reset_named_wins() {
        assert_eq!(at("resets 1pm · resets 2pm").as_deref(), Some("2026-11-24 14:00"));
    }
}
