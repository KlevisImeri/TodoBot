use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone)]
pub struct EventDraft {
    pub name: String,
    pub description: Option<String>,
    pub location: String,
    pub start: DateTime<Utc>,
    pub end: DateTime<Utc>,
    pub all_day: bool,
    pub recurrence: Option<DiscordRecurrenceRule>,
    pub recurrence_label: Option<String>,
}

impl EventDraft {
    pub fn payload(&self) -> CreateScheduledEventPayload {
        CreateScheduledEventPayload {
            name: self.name.clone(),
            description: self.description.clone(),
            scheduled_start_time: format_ts(self.start),
            scheduled_end_time: format_ts(self.end),
            privacy_level: 2,
            entity_type: 3,
            entity_metadata: EntityMetadata {
                location: self.location.clone(),
            },
            recurrence_rule: self.recurrence.clone(),
        }
    }
}

pub fn format_ts(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub fn truncate(value: &str, max: usize) -> String {
    if max == 0 {
        return String::new();
    }
    if value.chars().count() <= max {
        return value.to_string();
    }
    let mut result: String = value.chars().take(max - 1).collect();
    result.push('…');
    result
}

/// Normalises the escaped text used by iCalendar.
///
/// The `icalendar` parser already unescapes most values, but some calendar
/// exporters leave literal escape sequences in place.
pub fn clean_text(value: &str) -> String {
    value
        .replace("\\r\\n", "\n")
        .replace("\\n", "\n")
        .replace("\\N", "\n")
        .replace("\\,", ",")
        .replace("\\;", ";")
        .replace("\\\\", "\\")
        .trim()
        .to_string()
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DiscordRecurrenceRule {
    pub start: String,
    pub frequency: u8,
    pub interval: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_weekday: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_n_weekday: Option<Vec<DiscordNWeekday>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_month: Option<Vec<u8>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub by_month_day: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DiscordNWeekday {
    pub n: u8,
    pub day: u8,
}

#[derive(Debug, Serialize, PartialEq)]
pub struct CreateScheduledEventPayload {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub scheduled_start_time: String,
    pub scheduled_end_time: String,
    pub privacy_level: u8,
    pub entity_type: u8,
    pub entity_metadata: EntityMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recurrence_rule: Option<DiscordRecurrenceRule>,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct EntityMetadata {
    pub location: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScheduledEvent {
    pub id: String,
    #[serde(default)]
    pub status: u8,
}

#[derive(Debug, Clone, Deserialize)]
pub struct PartialGuild {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn truncation_keeps_char_boundaries() {
        assert_eq!(truncate("short", 10), "short");
        assert_eq!(truncate("abcdef", 4), "abc…");
        assert_eq!(truncate("abc", 0), "");
    }

    #[test]
    fn clean_text_handles_ical_escapes() {
        assert_eq!(clean_text("line one\\nline two"), "line one\nline two");
        assert_eq!(clean_text("a\\,b\\;c"), "a,b;c");
    }

    #[test]
    fn formats_timestamps_as_utc() {
        let value = Utc.with_ymd_and_hms(2026, 11, 2, 9, 0, 0).unwrap();
        assert_eq!(format_ts(value), "2026-11-02T09:00:00Z");
    }
}
