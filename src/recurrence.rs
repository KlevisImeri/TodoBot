use chrono::{DateTime, Datelike, Utc};
use chrono_tz::Tz;
use icalendar::CalendarEvent;
use rrule::Tz as RruleTz;

use crate::model::{format_ts, DiscordNWeekday, DiscordRecurrenceRule};

pub struct RecurrenceMapping {
    pub rule: DiscordRecurrenceRule,
    pub label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ByDay {
    n: Option<i32>,
    day: u8,
}

#[derive(Debug, Clone, Copy)]
enum Frequency {
    Yearly,
    Monthly,
    Weekly,
    Daily,
}

/// Returns the first occurrence strictly after `after`.
pub fn next_occurrence(
    cal_event: &CalendarEvent<'_>,
    after: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let set = cal_event.get_recurrence().ok()?;
    let after = after.with_timezone(&RruleTz::UTC);
    let result = set.after(after).all(1);
    result.dates.first().map(|date| date.with_timezone(&Utc))
}

/// Translates the subset of iCalendar RRULE that Discord can represent.
pub fn map_rrule(raw: &str, start: DateTime<Tz>) -> Result<RecurrenceMapping, String> {
    let raw = raw.trim().strip_prefix("RRULE:").unwrap_or(raw).trim();
    if raw.is_empty() {
        return Err("empty recurrence rule".to_string());
    }

    let mut frequency: Option<Frequency> = None;
    let mut interval = 1u32;
    let mut by_day: Vec<ByDay> = Vec::new();
    let mut by_month: Vec<u8> = Vec::new();
    let mut by_month_day: Vec<i32> = Vec::new();

    for part in raw.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (key, value) = part
            .split_once('=')
            .ok_or_else(|| format!("invalid RRULE part: {part}"))?;
        match key.trim().to_ascii_uppercase().as_str() {
            "FREQ" => {
                if frequency.is_some() {
                    return Err("RRULE contains more than one FREQ".to_string());
                }
                frequency = Some(match value.trim().to_ascii_uppercase().as_str() {
                    "YEARLY" => Frequency::Yearly,
                    "MONTHLY" => Frequency::Monthly,
                    "WEEKLY" => Frequency::Weekly,
                    "DAILY" => Frequency::Daily,
                    other => return Err(format!("unsupported frequency {other}")),
                });
            }
            "INTERVAL" => {
                interval = value
                    .trim()
                    .parse::<u32>()
                    .map_err(|_| format!("invalid INTERVAL: {value}"))?;
                if interval == 0 {
                    return Err("INTERVAL must be positive".to_string());
                }
            }
            "BYDAY" => by_day = parse_by_day(value)?,
            "BYMONTH" => by_month = parse_by_month(value)?,
            "BYMONTHDAY" => by_month_day = parse_by_month_day(value)?,
            "COUNT" | "UNTIL" => {
                return Err("COUNT/UNTIL cannot be represented by Discord".to_string())
            }
            // Week start does not affect the simple one-weekday rules we map.
            "WKST" => {}
            other => return Err(format!("unsupported RRULE field {other}")),
        }
    }

    let frequency = frequency.ok_or_else(|| "RRULE has no FREQ".to_string())?;

    match frequency {
        Frequency::Weekly if interval == 1 || interval == 2 => {}
        Frequency::Weekly => {
            return Err("Discord only supports a weekly interval of 1 or 2".to_string())
        }
        _ if interval == 1 => {}
        _ => return Err("Discord cannot represent this interval".to_string()),
    }

    let rule = match frequency {
        Frequency::Daily => {
            if !by_month.is_empty() || !by_month_day.is_empty() {
                return Err("BYMONTH/BYMONTHDAY are not supported for daily events".to_string());
            }
            let mut weekdays = Vec::new();
            for entry in &by_day {
                if entry.n.is_some() {
                    return Err("ordinal weekdays are not supported for daily events".to_string());
                }
                weekdays.push(entry.day);
            }
            weekdays.sort_unstable();
            weekdays.dedup();
            let by_weekday = if weekdays.is_empty() || weekdays == vec![0, 1, 2, 3, 4, 5, 6] {
                None
            } else {
                if !is_allowed_daily_set(&weekdays) {
                    return Err(
                        "Discord only supports specific weekday sets for daily events".to_string(),
                    );
                }
                Some(weekdays)
            };
            let label = daily_label(by_weekday.as_deref());
            DiscordRecurrenceRule {
                start: format_ts(start.with_timezone(&Utc)),
                frequency: 3,
                interval,
                by_weekday,
                by_n_weekday: None,
                by_month: None,
                by_month_day: None,
            }
            .into_mapping(label)
        }
        Frequency::Weekly => {
            if !by_month.is_empty() || !by_month_day.is_empty() {
                return Err("BYMONTH/BYMONTHDAY are not supported for weekly events".to_string());
            }
            if by_day.len() > 1 {
                return Err("Discord only supports one weekday for a weekly event".to_string());
            }
            let day = match by_day.first() {
                Some(entry) => {
                    if entry.n.is_some() {
                        return Err(
                            "ordinal weekdays are not supported for weekly events".to_string()
                        );
                    }
                    entry.day
                }
                None => start.weekday().num_days_from_monday() as u8,
            };
            let label = if interval == 2 {
                format!("Every other {}", weekday_name(day))
            } else {
                format!("Every {}", weekday_name(day))
            };
            DiscordRecurrenceRule {
                start: format_ts(start.with_timezone(&Utc)),
                frequency: 2,
                interval,
                by_weekday: Some(vec![day]),
                by_n_weekday: None,
                by_month: None,
                by_month_day: None,
            }
            .into_mapping(label)
        }
        Frequency::Monthly => {
            if !by_month.is_empty() || !by_month_day.is_empty() {
                return Err("Discord cannot represent monthly events by month/day".to_string());
            }
            let entry = match by_day.as_slice() {
                [entry] => entry,
                _ => {
                    return Err(
                        "Discord monthly events need one ordinal weekday like 4WE".to_string()
                    )
                }
            };
            let n = entry
                .n
                .ok_or_else(|| "Discord monthly events need an ordinal weekday".to_string())?;
            if !(1..=5).contains(&n) {
                return Err(
                    "Discord supports only the 1st through 5th weekday of a month".to_string(),
                );
            }
            let label = format!(
                "Monthly on the {} {}",
                ordinal(n as u8),
                weekday_name(entry.day)
            );
            DiscordRecurrenceRule {
                start: format_ts(start.with_timezone(&Utc)),
                frequency: 1,
                interval,
                by_weekday: None,
                by_n_weekday: Some(vec![DiscordNWeekday {
                    n: n as u8,
                    day: entry.day,
                }]),
                by_month: None,
                by_month_day: None,
            }
            .into_mapping(label)
        }
        Frequency::Yearly => {
            if !by_day.is_empty() {
                return Err(
                    "Discord cannot represent yearly events selected by weekday".to_string()
                );
            }
            let months = if by_month.is_empty() {
                vec![start.month() as u8]
            } else {
                by_month
            };
            let days = if by_month_day.is_empty() {
                vec![start.day() as u8]
            } else {
                let mut days = Vec::new();
                for day in by_month_day {
                    if !(1..=31).contains(&day) {
                        return Err("Discord requires a positive day of month".to_string());
                    }
                    days.push(day as u8);
                }
                days
            };
            if months.len() != 1 || days.len() != 1 {
                return Err("Discord supports one month/day pair for yearly events".to_string());
            }
            let label = format!("Yearly on {} {}", month_name(months[0]), days[0]);
            DiscordRecurrenceRule {
                start: format_ts(start.with_timezone(&Utc)),
                frequency: 0,
                interval,
                by_weekday: None,
                by_n_weekday: None,
                by_month: Some(months),
                by_month_day: Some(days),
            }
            .into_mapping(label)
        }
    };

    Ok(rule)
}

trait IntoMapping {
    fn into_mapping(self, label: String) -> RecurrenceMapping;
}

impl IntoMapping for DiscordRecurrenceRule {
    fn into_mapping(self, label: String) -> RecurrenceMapping {
        RecurrenceMapping { rule: self, label }
    }
}

fn parse_by_day(value: &str) -> Result<Vec<ByDay>, String> {
    value
        .split(',')
        .map(|token| {
            let token = token.trim().to_ascii_uppercase();
            if token.len() < 2 {
                return Err(format!("invalid BYDAY value: {token}"));
            }
            let (prefix, day_code) = token.split_at(token.len() - 2);
            let day = match day_code {
                "MO" => 0,
                "TU" => 1,
                "WE" => 2,
                "TH" => 3,
                "FR" => 4,
                "SA" => 5,
                "SU" => 6,
                _ => return Err(format!("invalid weekday: {day_code}")),
            };
            let n = if prefix.is_empty() {
                None
            } else {
                Some(
                    prefix
                        .parse::<i32>()
                        .map_err(|_| format!("invalid weekday ordinal: {prefix}"))?,
                )
            };
            Ok(ByDay { n, day })
        })
        .collect()
}

fn parse_by_month(value: &str) -> Result<Vec<u8>, String> {
    value
        .split(',')
        .map(|token| {
            let month = token
                .trim()
                .parse::<u8>()
                .map_err(|_| format!("invalid BYMONTH value: {token}"))?;
            if !(1..=12).contains(&month) {
                return Err(format!("invalid month: {month}"));
            }
            Ok(month)
        })
        .collect()
}

fn parse_by_month_day(value: &str) -> Result<Vec<i32>, String> {
    value
        .split(',')
        .map(|token| {
            let day = token
                .trim()
                .parse::<i32>()
                .map_err(|_| format!("invalid BYMONTHDAY value: {token}"))?;
            if day == 0 || !(1..=31).contains(&day.abs()) {
                return Err(format!("invalid day of month: {day}"));
            }
            Ok(day)
        })
        .collect()
}

fn is_allowed_daily_set(days: &[u8]) -> bool {
    matches!(
        days,
        [0, 1, 2, 3, 4] | [1, 2, 3, 4, 5] | [0, 1, 2, 3, 6] | [4, 5] | [5, 6] | [0, 6]
    )
}

fn daily_label(days: Option<&[u8]>) -> String {
    match days {
        None => "Every day".to_string(),
        Some(days) if days == &[0, 1, 2, 3, 4][..] => "Every weekday".to_string(),
        Some(days) if days == &[1, 2, 3, 4, 5][..] => "Tuesday to Saturday".to_string(),
        Some(days) if days == &[0, 1, 2, 3, 6][..] => "Sunday to Thursday".to_string(),
        Some(days) if days == &[4, 5][..] => "Friday and Saturday".to_string(),
        Some(days) if days == &[5, 6][..] => "Saturday and Sunday".to_string(),
        Some(days) if days == &[0, 6][..] => "Sunday and Monday".to_string(),
        Some(days) => format!(
            "Daily on {}",
            days.iter()
                .map(|day| weekday_name(*day))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

fn weekday_name(day: u8) -> &'static str {
    match day {
        0 => "Monday",
        1 => "Tuesday",
        2 => "Wednesday",
        3 => "Thursday",
        4 => "Friday",
        5 => "Saturday",
        6 => "Sunday",
        _ => "Unknown",
    }
}

fn month_name(month: u8) -> &'static str {
    match month {
        1 => "January",
        2 => "February",
        3 => "March",
        4 => "April",
        5 => "May",
        6 => "June",
        7 => "July",
        8 => "August",
        9 => "September",
        10 => "October",
        11 => "November",
        12 => "December",
        _ => "Unknown",
    }
}

fn ordinal(n: u8) -> &'static str {
    match n {
        1 => "1st",
        2 => "2nd",
        3 => "3rd",
        4 => "4th",
        _ => "5th",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use chrono_tz::Tz;

    fn monday() -> DateTime<Tz> {
        Tz::Europe__Zurich
            .with_ymd_and_hms(2026, 11, 2, 10, 0, 0)
            .single()
            .unwrap()
    }

    fn wednesday() -> DateTime<Tz> {
        Tz::Europe__Zurich
            .with_ymd_and_hms(2026, 11, 4, 10, 0, 0)
            .single()
            .unwrap()
    }

    #[test]
    fn maps_weekly() {
        let mapping = map_rrule("FREQ=WEEKLY;BYDAY=MO", monday()).unwrap();
        assert_eq!(mapping.rule.frequency, 2);
        assert_eq!(mapping.rule.interval, 1);
        assert_eq!(mapping.rule.by_weekday, Some(vec![0]));
        assert_eq!(mapping.label, "Every Monday");
    }

    #[test]
    fn maps_every_other_week() {
        let mapping = map_rrule("FREQ=WEEKLY;INTERVAL=2;BYDAY=WE", wednesday()).unwrap();
        assert_eq!(mapping.rule.interval, 2);
        assert_eq!(mapping.rule.by_weekday, Some(vec![2]));
        assert_eq!(mapping.label, "Every other Wednesday");
    }

    #[test]
    fn maps_weekdays() {
        let mapping = map_rrule("FREQ=DAILY;BYDAY=MO,TU,WE,TH,FR", monday()).unwrap();
        assert_eq!(mapping.rule.frequency, 3);
        assert_eq!(mapping.rule.by_weekday, Some(vec![0, 1, 2, 3, 4]));
        assert_eq!(mapping.label, "Every weekday");
    }

    #[test]
    fn maps_monthly_nth_weekday() {
        let mapping = map_rrule("FREQ=MONTHLY;BYDAY=4WE", wednesday()).unwrap();
        assert_eq!(mapping.rule.frequency, 1);
        assert_eq!(
            mapping.rule.by_n_weekday,
            Some(vec![DiscordNWeekday { n: 4, day: 2 }])
        );
        assert_eq!(mapping.label, "Monthly on the 4th Wednesday");
    }

    #[test]
    fn maps_yearly_date() {
        let start = Tz::Europe__Zurich
            .with_ymd_and_hms(2026, 7, 24, 10, 0, 0)
            .single()
            .unwrap();
        let mapping = map_rrule("FREQ=YEARLY;BYMONTH=7;BYMONTHDAY=24", start).unwrap();
        assert_eq!(mapping.rule.frequency, 0);
        assert_eq!(mapping.rule.by_month, Some(vec![7]));
        assert_eq!(mapping.rule.by_month_day, Some(vec![24]));
        assert_eq!(mapping.label, "Yearly on July 24");
    }

    #[test]
    fn rejects_monthly_by_month_day() {
        assert!(map_rrule("FREQ=MONTHLY;BYMONTHDAY=15", monday()).is_err());
    }

    #[test]
    fn rejects_weekly_multiple_days() {
        assert!(map_rrule("FREQ=WEEKLY;BYDAY=MO,WE", monday()).is_err());
    }

    #[test]
    fn rejects_count_and_until() {
        assert!(map_rrule("FREQ=WEEKLY;BYDAY=MO;COUNT=5", monday()).is_err());
        assert!(map_rrule("FREQ=WEEKLY;BYDAY=MO;UNTIL=20261231T235959Z", monday()).is_err());
    }

    #[test]
    fn finds_next_occurrence() {
        let calendar: icalendar::Calendar = "\
BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:weekly
DTSTART:20261102T100000Z
DTEND:20261102T110000Z
RRULE:FREQ=WEEKLY;BYDAY=MO
SUMMARY:Weekly sync
END:VEVENT
END:VCALENDAR
"
        .parse()
        .unwrap();

        let event = calendar.calendar_events().next().unwrap();
        let after = Utc.with_ymd_and_hms(2026, 11, 3, 0, 0, 0).unwrap();
        let next = next_occurrence(&event, after).unwrap();
        assert_eq!(next, Utc.with_ymd_and_hms(2026, 11, 9, 10, 0, 0).unwrap());
    }

    #[test]
    fn exhausted_series_has_no_next_occurrence() {
        let calendar: icalendar::Calendar = "\
BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:once
DTSTART:20261102T100000Z
DTEND:20261102T110000Z
RRULE:FREQ=WEEKLY;BYDAY=MO;COUNT=1
SUMMARY:One off
END:VEVENT
END:VCALENDAR
"
        .parse()
        .unwrap();

        let event = calendar.calendar_events().next().unwrap();
        let after = Utc.with_ymd_and_hms(2026, 11, 3, 0, 0, 0).unwrap();
        assert!(next_occurrence(&event, after).is_none());
    }
}
