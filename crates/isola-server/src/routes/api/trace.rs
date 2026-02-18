use std::sync::atomic::AtomicU64;

use isola_trace::collect::{Collector, EventRecord, FieldFilter, SpanRecord};
use serde::Serialize;
use tokio::sync::mpsc;

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

pub struct HttpTraceCollector {
    tx: mpsc::UnboundedSender<HttpTrace>,
    idgen: AtomicU64,
}

impl HttpTraceCollector {
    pub fn new() -> (Self, mpsc::UnboundedReceiver<HttpTrace>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                tx,
                idgen: AtomicU64::new(1),
            },
            rx,
        )
    }

    fn next_id(&self) -> u64 {
        self.idgen
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    fn format_timestamp(time: std::time::SystemTime) -> String {
        let duration = time
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        let secs = duration.as_secs();
        let nanos = duration.subsec_nanos();

        let days_since_epoch = secs / 86400;
        let time_of_day = secs % 86400;

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

fn group_name(name: &'static str) -> String {
    name.find('.')
        .map(|i| name[..i].to_string())
        .unwrap_or_default()
}

impl Collector for HttpTraceCollector {
    fn on_span_start(&self, span: SpanRecord) {
        let time = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_nanos(span.begin_time_unix_ns);
        let span_id = to_i64(span.span_id);

        let _ = self.tx.send(HttpTrace {
            id: span_id,
            group: group_name(span.name),
            timestamp: Self::format_timestamp(time),
            data: TraceData::SpanBegin {
                name: span.name.to_string(),
            },
        });
    }

    fn on_span_end(&self, span: SpanRecord) {
        let time = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_nanos(span.begin_time_unix_ns);
        let end = time + std::time::Duration::from_nanos(span.duration_ns);
        let span_id = to_i64(span.span_id);

        let _ = self.tx.send(HttpTrace {
            id: to_i64(self.next_id()),
            group: group_name(span.name),
            timestamp: Self::format_timestamp(end),
            data: TraceData::SpanEnd { parent_id: span_id },
        });
    }

    fn on_event(&self, event: EventRecord) {
        let time = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_nanos(event.timestamp_unix_ns);
        let id = to_i64(self.next_id());

        if event.name == "log" {
            let message = event
                .properties
                .iter()
                .find_map(|(k, v)| {
                    if *k == "log.output" {
                        Some(v.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_default();

            let context = event
                .properties
                .iter()
                .find_map(|(k, v)| {
                    if *k == "log.context" {
                        Some(v.clone())
                    } else {
                        None
                    }
                })
                .unwrap_or_default();

            let level = if context.is_empty() {
                "INFO".to_string()
            } else {
                context.to_uppercase()
            };

            let _ = self.tx.send(HttpTrace {
                id,
                group: group_name(event.name),
                timestamp: Self::format_timestamp(time),
                data: TraceData::Log { level, message },
            });
        }
    }

    fn next_id(&self) -> u64 {
        self.idgen
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    }

    fn field_filter() -> Option<FieldFilter>
    where
        Self: Sized,
    {
        Some(FieldFilter {
            ignore_prefix: &["otel."],
        })
    }
}
