use std::sync::atomic::AtomicU64;

use promptkit_trace::collect::{Collector, EventRecord, FieldFilter, SpanRecord};
use tokio::sync::mpsc;

use crate::proto::script::v1::{
    Trace,
    trace::{Event, Log, SpanBegin, SpanEnd, TraceType},
};

pub struct TraceCollector {
    tx: mpsc::UnboundedSender<Trace>,
    idgen: AtomicU64,
}

impl TraceCollector {
    pub fn new() -> (Self, mpsc::UnboundedReceiver<Trace>) {
        let (tx, rx) = mpsc::unbounded_channel();
        (
            Self {
                tx,
                idgen: AtomicU64::new(1),
            },
            rx,
        )
    }
}

impl Collector for TraceCollector {
    fn collect_span_start(&self, span: SpanRecord) {
        let time = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_nanos(span.begin_time_unix_ns);
        let span_id = to_i32(span.span_id);
        let parent_id = to_i32(span.parent_id);

        let _ = self.tx.send(Trace {
            id: span_id,
            group: group_name(span.name),
            timestamp: Some(prost_types::Timestamp::from(time)),
            trace_type: Some(TraceType::SpanBegin(SpanBegin {
                kind: span.name.to_string(),
                parent_id,
                data: span
                    .properties
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
            })),
        });
    }

    fn collect_span_end(&self, span: SpanRecord) {
        let time = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_nanos(span.begin_time_unix_ns);
        let end = time + std::time::Duration::from_nanos(span.duration_ns);
        let span_id = to_i32(span.span_id);

        let _ = self.tx.send(Trace {
            id: to_i32(self.next_id()),
            group: group_name(span.name),
            timestamp: Some(prost_types::Timestamp::from(end)),
            trace_type: Some(TraceType::SpanEnd(SpanEnd {
                parent_id: span_id,
                data: span
                    .properties
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
            })),
        });
    }

    fn collect_event(&self, event: EventRecord) {
        let time = std::time::SystemTime::UNIX_EPOCH
            + std::time::Duration::from_nanos(event.timestamp_unix_ns);
        let id = to_i32(self.next_id());
        if event.name == "log" {
            let _ = self.tx.send(Trace {
                id,
                group: group_name(event.name),
                timestamp: Some(prost_types::Timestamp::from(time)),
                trace_type: Some(TraceType::Log(Log {
                    content: event
                        .properties
                        .iter()
                        .find_map(|(k, v)| if *k == "log.output" { Some(v) } else { None })
                        .cloned()
                        .unwrap_or_default(),
                    context: event
                        .properties
                        .iter()
                        .find_map(|(k, v)| if *k == "log.context" { Some(v) } else { None })
                        .cloned()
                        .unwrap_or_default(),
                })),
            });
        } else {
            let _ = self.tx.send(Trace {
                id,
                group: group_name(event.name),
                timestamp: Some(prost_types::Timestamp::from(time)),
                trace_type: Some(TraceType::Event(Event {
                    kind: event.name.to_string(),
                    parent_id: to_i32(event.parent_span_id),
                    data: event
                        .properties
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v))
                        .collect(),
                })),
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

#[inline]
fn to_i32(value: u64) -> i32 {
    i32::try_from(value).unwrap_or_default()
}

fn group_name(name: &'static str) -> String {
    name.find('.')
        .map(|i| name[..i].to_string())
        .unwrap_or_default()
}
