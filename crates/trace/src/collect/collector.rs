use std::cell::Cell;

pub trait Collector: Sync + Send + 'static {
    fn collect_span_start(&self, span: SpanRecord);
    fn collect_span_end(&self, span: SpanRecord);
    fn collect_event(&self, event: EventRecord);
    fn next_id(&self) -> u64 {
        LOCAL_ID_GENERATOR
            .try_with(|g| {
                let (prefix, mut suffix) = g.get();

                suffix = suffix.wrapping_add(1);

                g.set((prefix, suffix));

                ((prefix as u64) << 32) | (suffix as u64)
            })
            .unwrap_or_else(|_| rand::random())
    }

    #[must_use]
    fn field_filter() -> Option<FieldFilter>
    where
        Self: Sized,
    {
        None
    }
}

thread_local! {
    static LOCAL_ID_GENERATOR: Cell<(u32, u32)> = Cell::new((rand::random(), 0))
}

#[derive(Clone, Debug)]
pub struct SpanRecord {
    pub span_id: u64,
    pub parent_id: u64,
    pub begin_time_unix_ns: u64,
    pub duration_ns: u64,
    pub name: &'static str,
    pub properties: Vec<(&'static str, String)>,
}

#[derive(Clone, Debug)]
pub struct EventRecord {
    pub parent_span_id: u64,
    pub name: &'static str,
    pub timestamp_unix_ns: u64,
    pub properties: Vec<(&'static str, String)>,
}

pub struct FieldFilter {
    pub ignore_prefix: &'static [&'static str],
}

impl FieldFilter {
    #[must_use]
    #[inline]
    pub fn enabled(&self, name: &'static str) -> bool {
        !self
            .ignore_prefix
            .iter()
            .any(|prefix| name.starts_with(prefix))
    }
}
