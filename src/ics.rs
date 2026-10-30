use anyhow::{anyhow, bail, Result};
use chrono::{DateTime, Duration, LocalResult, NaiveDate, NaiveDateTime, NaiveTime, TimeZone, Utc};
use chrono_tz::Tz;
use icalendar::{
    Calendar, CalendarDateTime, CalendarEvent, Component, DatePerhapsTime, EventLike, EventStatus,
};
use std::str::FromStr;

use crate::model::{clean_text, truncate, EventDraft};
use crate::recurrence::{map_rrule, next_occurrence};

pub struct ParseOptions {
    pub default_timezone: Tz,
    pub now: DateTime<Utc>,
}

#[derive(Default)]
pub struct ParsedCalendar {
    pub events: Vec<EventDraft>,
    pub warnings: Vec<String>,
    pub skipped: Vec<String>,
}

#[derive(Debug, Clone)]
struct TimeWindow {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    all_day: bool,
    duration_days: i64,
    duration: Duration,
    source_tz: Tz,
}

impl TimeWindow {
    fn at_occurrence(&self, occurrence: DateTime<Utc>) -> Self {
        if self.all_day {
            let date = occurrence.with_timezone(&self.source_tz).date_naive();
            let start = local_midnight(date, self.source_tz);
            let end = local_midnight(date + Duration::days(self.duration_days), self.source_tz);
            Self {
                start,
                end,
                all_day: true,
                duration_days: self.duration_days,
                duration: end - start,
                source_tz: self.source_tz,
            }
        } else {
            let start = occurrence;
            let end = start + self.duration;
            Self {
                start,
                end,
                all_day: false,
                duration_days: 0,
                duration: self.duration,
                source_tz: self.source_tz,
            }
        }
    }

    fn start_local(&self) -> DateTime<Tz> {
        self.start.with_timezone(&self.source_tz)
    }
}

pub fn parse_calendar(content: &str, opts: &ParseOptions) -> Result<ParsedCalendar> {
    let content = content.strip_prefix('\u{feff}').unwrap_or(content);
    let calendar: Calendar = content
        .parse()
        .map_err(|error| anyhow!("failed to parse the .ics file: {error}"))?;
    let calendar_events: Vec<CalendarEvent<'_>> = calendar.calendar_events().collect();
    if calendar_events.is_empty() {
        bail!("no VEVENT components found (VTODO/VJOURNAL are not supported)");
    }

    let mut parsed = ParsedCalendar::default();

    for cal_event in calendar_events {
        let event = cal_event.event();
        let raw_name = event
            .get_summary()
            .map(clean_text)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Untitled event".to_string());
        let name = truncate(&raw_name, 100);

        if matches!(event.get_status(), Some(EventStatus::Cancelled)) {
            parsed.skipped.push(format!("{name}: cancelled"));
            continue;
        }
        if event.property_value("RECURRENCE-ID").is_some() {
            parsed.skipped.push(format!("{name}: recurring override"));
            continue;
        }

        let Some(start_spec) = event.get_start() else {
            parsed.skipped.push(format!("{name}: missing DTSTART"));
            continue;
        };
        let end_spec = event.get_end();
        let mut event_warnings = Vec::new();
        let Some(window) = time_window(
            &start_spec,
            end_spec.as_ref(),
            event.property_value("DURATION"),
            opts.default_timezone,
            &name,
            &mut event_warnings,
        ) else {
            parsed
                .skipped
                .push(format!("{name}: invalid start/end time"));
            parsed.warnings.extend(event_warnings);
            continue;
        };

        let rrule = event
            .property_value("RRULE")
            .map(str::trim)
            .filter(|value| !value.is_empty());
        let has_exceptions =
            event.property_value("RDATE").is_some() || event.property_value("EXDATE").is_some();

        let (start, end, recurrence, recurrence_label) = if let Some(rrule) = rrule {
            let Some(next) = next_occurrence(&cal_event, opts.now) else {
                parsed
                    .skipped
                    .push(format!("{name}: recurrence has no upcoming occurrences"));
                parsed.warnings.extend(event_warnings);
                continue;
            };
            let next_window = window.at_occurrence(next);
            let (recurrence, recurrence_label) = if has_exceptions {
                event_warnings.push(
                    "RDATE/EXDATE exceptions are not supported; created only the next occurrence"
                        .to_string(),
                );
                (None, None)
            } else {
                match map_rrule(rrule, next_window.start_local()) {
                    Ok(mapping) => (Some(mapping.rule), Some(mapping.label)),
                    Err(reason) => {
                        event_warnings.push(format!(
                            "recurrence not mapped by Discord ({reason}); created only the next occurrence"
                        ));
                        (None, None)
                    }
                }
            };
            (
                next_window.start,
                next_window.end,
                recurrence,
                recurrence_label,
            )
        } else {
            if window.start <= opts.now {
                parsed
                    .skipped
                    .push(format!("{name}: already started or in the past"));
                continue;
            }
            (window.start, window.end, None, None)
        };

        let description = event
            .get_description()
            .map(clean_text)
            .filter(|value| !value.is_empty())
            .map(|value| truncate(&value, 1000));
        let location = event
            .get_location()
            .map(clean_text)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "Imported from ICS".to_string());
        let location = truncate(&location, 100);
        let all_day = window.all_day;

        parsed.events.push(EventDraft {
            name,
            description,
            location,
            start,
            end,
            all_day,
            recurrence,
            recurrence_label,
        });
        parsed.warnings.extend(event_warnings);
    }

    Ok(parsed)
}

fn time_window(
    start_spec: &DatePerhapsTime,
    end_spec: Option<&DatePerhapsTime>,
    duration_raw: Option<&str>,
    default_tz: Tz,
    name: &str,
    warnings: &mut Vec<String>,
) -> Option<TimeWindow> {
    let (start_utc, source_tz, all_day) = match start_spec {
        DatePerhapsTime::Date(date) => (local_midnight(*date, default_tz), default_tz, true),
        DatePerhapsTime::DateTime(datetime) => {
            let (start, tz) = parse_datetime(datetime, default_tz, name, warnings)?;
            (start, tz, false)
        }
    };

    if all_day {
        let start_date = start_utc.with_timezone(&source_tz).date_naive();
        let end_date = match end_spec {
            Some(DatePerhapsTime::Date(date)) => *date,
            Some(DatePerhapsTime::DateTime(datetime)) => {
                let (end, _) = parse_datetime(datetime, default_tz, name, warnings)?;
                end.with_timezone(&source_tz).date_naive()
            }
            None => {
                let days = duration_raw
                    .and_then(parse_duration)
                    .map(|duration| duration.num_days())
                    .filter(|days| *days > 0)
                    .unwrap_or(1);
                start_date + Duration::days(days)
            }
        };

        let mut duration_days = (end_date - start_date).num_days();
        if duration_days <= 0 {
            warnings.push(format!("{name}: invalid all-day end; using one day"));
            duration_days = 1;
        }
        let start = local_midnight(start_date, source_tz);
        let end = local_midnight(start_date + Duration::days(duration_days), source_tz);
        Some(TimeWindow {
            start,
            end,
            all_day: true,
            duration_days,
            duration: end - start,
            source_tz,
        })
    } else {
        let end = match end_spec {
            Some(DatePerhapsTime::DateTime(datetime)) => {
                parse_datetime(datetime, default_tz, name, warnings)?.0
            }
            Some(DatePerhapsTime::Date(date)) => local_midnight(*date, default_tz),
            None => {
                if let Some(raw) = duration_raw {
                    match parse_duration(raw) {
                        Some(duration) => start_utc + duration,
                        None => {
                            warnings.push(format!("{name}: invalid DURATION; using one hour"));
                            start_utc + Duration::hours(1)
                        }
                    }
                } else {
                    start_utc + Duration::hours(1)
                }
            }
        };

        let mut end = end;
        if end <= start_utc {
            warnings.push(format!("{name}: end is before start; using one hour"));
            end = start_utc + Duration::hours(1);
        }
        let duration = end - start_utc;
        Some(TimeWindow {
            start: start_utc,
            end,
            all_day: false,
            duration_days: 0,
            duration,
            source_tz,
        })
    }
}

fn parse_datetime(
    value: &CalendarDateTime,
    default_tz: Tz,
    name: &str,
    warnings: &mut Vec<String>,
) -> Option<(DateTime<Utc>, Tz)> {
    match value {
        CalendarDateTime::Utc(datetime) => Some((*datetime, default_tz)),
        CalendarDateTime::Floating(naive) => Some((
            local_to_utc(*naive, default_tz, name, warnings)?,
            default_tz,
        )),
        CalendarDateTime::WithTimezone { date_time, tzid } => {
            let tz = Tz::from_str(tzid).unwrap_or_else(|_| {
                warnings.push(format!(
                    "{name}: unknown timezone {tzid}; using {default_tz}"
                ));
                default_tz
            });
            Some((local_to_utc(*date_time, tz, name, warnings)?, tz))
        }
    }
}

fn local_to_utc(
    naive: NaiveDateTime,
    tz: Tz,
    name: &str,
    warnings: &mut Vec<String>,
) -> Option<DateTime<Utc>> {
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(value) => Some(value.with_timezone(&Utc)),
        LocalResult::Ambiguous(first, _) => {
            warnings.push(format!(
                "{name}: ambiguous local time; using the earlier offset"
            ));
            Some(first.with_timezone(&Utc))
        }
        LocalResult::None => {
            warnings.push(format!(
                "{name}: local time does not exist in {tz}; skipped"
            ));
            None
        }
    }
}

fn local_midnight(date: NaiveDate, tz: Tz) -> DateTime<Utc> {
    let naive = date.and_time(NaiveTime::MIN);
    match tz.from_local_datetime(&naive) {
        LocalResult::Single(value) => value.with_timezone(&Utc),
        LocalResult::Ambiguous(first, _) => first.with_timezone(&Utc),
        LocalResult::None => naive.and_utc(),
    }
}

fn parse_duration(raw: &str) -> Option<Duration> {
    let parsed: iso8601::Duration = raw.trim().parse().ok()?;
    match parsed {
        iso8601::Duration::Weeks(weeks) => Some(Duration::weeks(weeks as i64)),
        iso8601::Duration::YMDHMS {
            year,
            month,
            day,
            hour,
            minute,
            second,
            millisecond,
        } => {
            if year > 0 || month > 0 {
                return None;
            }
            let duration = Duration::days(day as i64)
                + Duration::hours(hour as i64)
                + Duration::minutes(minute as i64)
                + Duration::seconds(second as i64)
                + Duration::milliseconds(millisecond as i64);
            if duration <= Duration::zero() {
                None
            } else {
                Some(duration)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn options() -> ParseOptions {
        ParseOptions {
            default_timezone: Tz::Europe__Zurich,
            now: Utc.with_ymd_and_hms(2026, 10, 30, 12, 0, 0).unwrap(),
        }
    }

    #[test]
    fn parses_a_single_utc_event() {
        let parsed =
            parse_calendar(include_str!("../tests/fixtures/single.ics"), &options()).unwrap();
        assert_eq!(parsed.events.len(), 1);
        let event = &parsed.events[0];
        assert_eq!(event.name, "Team meeting");
        assert_eq!(event.location, "Office");
        assert!(event.description.as_deref().unwrap().contains("roadmap"));
        assert_eq!(
            event.start,
            Utc.with_ymd_and_hms(2026, 11, 2, 10, 0, 0).unwrap()
        );
        assert_eq!(
            event.end,
            Utc.with_ymd_and_hms(2026, 11, 2, 11, 0, 0).unwrap()
        );
    }

    #[test]
    fn parses_an_all_day_event_in_the_default_timezone() {
        let parsed =
            parse_calendar(include_str!("../tests/fixtures/all_day.ics"), &options()).unwrap();
        assert_eq!(parsed.events.len(), 1);
        let event = &parsed.events[0];
        assert!(event.all_day);
        assert_eq!(
            event.start,
            Tz::Europe__Zurich
                .with_ymd_and_hms(2026, 11, 2, 0, 0, 0)
                .single()
                .unwrap()
                .with_timezone(&Utc)
        );
        assert_eq!(
            event.end,
            Tz::Europe__Zurich
                .with_ymd_and_hms(2026, 11, 3, 0, 0, 0)
                .single()
                .unwrap()
                .with_timezone(&Utc)
        );
    }

    #[test]
    fn resolves_tzid_values() {
        let parsed =
            parse_calendar(include_str!("../tests/fixtures/tzid.ics"), &options()).unwrap();
        assert_eq!(parsed.events.len(), 1);
        assert_eq!(
            parsed.events[0].start,
            Utc.with_ymd_and_hms(2026, 11, 2, 9, 0, 0).unwrap()
        );
    }

    #[test]
    fn uses_duration_when_dtend_is_missing() {
        let calendar = "\
BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:duration
DTSTART:20261102T100000Z
DURATION:PT90M
SUMMARY:Duration event
END:VEVENT
END:VCALENDAR
";
        let parsed = parse_calendar(calendar, &options()).unwrap();
        assert_eq!(
            parsed.events[0].end - parsed.events[0].start,
            Duration::minutes(90)
        );
    }

    #[test]
    fn skips_cancelled_and_past_events() {
        let calendar = "\
BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:cancelled
DTSTART:20261102T100000Z
DTEND:20261102T110000Z
SUMMARY:Cancelled
STATUS:CANCELLED
END:VEVENT
BEGIN:VEVENT
UID:past
DTSTART:20261001T100000Z
DTEND:20261001T110000Z
SUMMARY:Past
END:VEVENT
BEGIN:VEVENT
UID:future
DTSTART:20261102T100000Z
DTEND:20261102T110000Z
SUMMARY:Future
END:VEVENT
END:VCALENDAR
";
        let parsed = parse_calendar(calendar, &options()).unwrap();
        assert_eq!(parsed.events.len(), 1);
        assert_eq!(parsed.events[0].name, "Future");
        assert_eq!(parsed.skipped.len(), 2);
    }

    #[test]
    fn imports_all_events() {
        let calendar = "\
BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:a
DTSTART:20261102T100000Z
DTEND:20261102T110000Z
SUMMARY:One
END:VEVENT
BEGIN:VEVENT
UID:b
DTSTART:20261103T100000Z
DTEND:20261103T110000Z
SUMMARY:Two
END:VEVENT
END:VCALENDAR
";
        let parsed = parse_calendar(calendar, &options()).unwrap();
        assert_eq!(parsed.events.len(), 2);
    }

    #[test]
    fn maps_a_recurring_event_to_the_next_occurrence() {
        let parsed =
            parse_calendar(include_str!("../tests/fixtures/recurring.ics"), &options()).unwrap();
        assert_eq!(parsed.events.len(), 1);
        let event = &parsed.events[0];
        assert_eq!(event.recurrence_label.as_deref(), Some("Every Monday"));
        assert!(event.recurrence.is_some());
        assert_eq!(
            event.start,
            Utc.with_ymd_and_hms(2026, 11, 2, 9, 0, 0).unwrap()
        );
    }

    #[test]
    fn falls_back_to_next_occurrence_for_unsupported_rules() {
        let calendar = "\
BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VEVENT
UID:monthly
DTSTART:20261015T100000Z
DTEND:20261015T110000Z
RRULE:FREQ=MONTHLY;BYMONTHDAY=15
SUMMARY:Monthly fifteenth
END:VEVENT
END:VCALENDAR
";
        let parsed = parse_calendar(calendar, &options()).unwrap();
        assert_eq!(parsed.events.len(), 1);
        assert!(parsed.events[0].recurrence.is_none());
        assert!(!parsed.warnings.is_empty());
        assert_eq!(
            parsed.events[0].start,
            Utc.with_ymd_and_hms(2026, 11, 15, 10, 0, 0).unwrap()
        );
    }

    #[test]
    fn rejects_calendar_without_vevents() {
        let calendar = "\
BEGIN:VCALENDAR
VERSION:2.0
BEGIN:VTODO
UID:todo
SUMMARY:A todo
END:VTODO
END:VCALENDAR
";
        assert!(parse_calendar(calendar, &options()).is_err());
    }
}
