use std::sync::atomic::{AtomicU64, Ordering};

use serde::Serialize;

pub const SCRIPT_EXEC_SPAN_NAME: &str = "script.exec";

#[derive(Debug, Clone, Serialize)]
pub struct HttpTrace {
    pub id: i64,
    pub group: String,
    pub timestamp: String,
    #[serde(flatten)]
    pub data: TraceData,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TraceData {
    Log { level: String, message: String },
    SpanBegin { name: String },
    SpanEnd { parent_id: i64 },
}

#[derive(Debug, Default)]
pub struct HttpTraceBuilder {
    idgen: AtomicU64,
}

impl HttpTraceBuilder {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            idgen: AtomicU64::new(1),
        }
    }

    pub fn span_begin(&self, name: &str) -> HttpTrace {
        let id = to_i64(self.next_id());
        HttpTrace {
            id,
            group: group_name(name),
            timestamp: Self::format_timestamp(std::time::SystemTime::now()),
            data: TraceData::SpanBegin {
                name: name.to_string(),
            },
        }
    }

    pub fn span_end(&self, name: &str, parent_id: i64) -> HttpTrace {
        HttpTrace {
            id: to_i64(self.next_id()),
            group: group_name(name),
            timestamp: Self::format_timestamp(std::time::SystemTime::now()),
            data: TraceData::SpanEnd { parent_id },
        }
    }

    pub fn log(&self, context: &str, message: impl Into<String>) -> HttpTrace {
        let level = if context.is_empty() {
            "INFO".to_string()
        } else {
            context.to_uppercase()
        };
        HttpTrace {
            id: to_i64(self.next_id()),
            group: group_name("log"),
            timestamp: Self::format_timestamp(std::time::SystemTime::now()),
            data: TraceData::Log {
                level,
                message: message.into(),
            },
        }
    }

    fn next_id(&self) -> u64 {
        self.idgen.fetch_add(1, Ordering::Relaxed)
    }

    fn format_timestamp(time: std::time::SystemTime) -> String {
        let duration = time
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = duration.as_secs();
        let nanos = duration.subsec_nanos();

        let days_since_epoch = secs / 86_400;
        let time_of_day = secs % 86_400;

        let mut year = 1970i32;
        let mut remaining_days = days_since_epoch;

        loop {
            let days_in_year = if is_leap_year(year) { 366 } else { 365 };
            if remaining_days < days_in_year {
                break;
            }
            remaining_days -= days_in_year;
            year += 1;
        }

        let leap = is_leap_year(year);
        let month_days = if leap {
            [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        } else {
            [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        };

        let mut month = 1u32;
        for days in month_days {
            if remaining_days < days {
                break;
            }
            remaining_days -= days;
            month += 1;
        }

        let day = remaining_days + 1;
        let hours = time_of_day / 3600;
        let minutes = (time_of_day % 3600) / 60;
        let seconds = time_of_day % 60;
        let millis = nanos / 1_000_000;

        format!("{year:04}-{month:02}-{day:02}T{hours:02}:{minutes:02}:{seconds:02}.{millis:03}Z")
    }
}

const fn is_leap_year(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || (year % 400 == 0)
}

#[inline]
fn to_i64(value: u64) -> i64 {
    i64::try_from(value).unwrap_or_default()
}

fn group_name(name: &str) -> String {
    name.find('.')
        .map(|i| name[..i].to_string())
        .unwrap_or_default()
}
