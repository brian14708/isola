use tracing::field::Visit;

pub(crate) struct FieldVisitor<'a> {
    name: &'static str,
    data: &'a mut String,
}

impl<'a> FieldVisitor<'a> {
    pub fn new(name: &'static str, data: &'a mut String) -> Self {
        FieldVisitor { name, data }
    }
}

macro_rules! record_value {
    ($self:expr, $field:expr, $value:expr) => {
        if $field.name() == $self.name {
            *$self.data = $value.to_string();
        }
    };
}

impl Visit for FieldVisitor<'_> {
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        record_value!(self, field, value);
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        record_value!(self, field, value);
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        record_value!(self, field, format!("{value:?}"));
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        record_value!(self, field, value);
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        record_value!(self, field, value);
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        record_value!(self, field, value);
    }

    fn record_i128(&mut self, field: &tracing::field::Field, value: i128) {
        record_value!(self, field, value);
    }

    fn record_u128(&mut self, field: &tracing::field::Field, value: u128) {
        record_value!(self, field, value);
    }
}

pub(crate) struct StringVisitor<F: FnMut(&'static str, String)> {
    f: F,
}

impl<F> StringVisitor<F>
where
    F: FnMut(&'static str, String),
{
    pub fn new(f: F) -> Self {
        StringVisitor { f }
    }
}

macro_rules! record_map_value {
    ($self:ident, $field:expr, $value:expr) => {
        let name = $field.name();
        if name.starts_with("promptkit.") || name.starts_with("otel.") {
            return;
        }
        ($self.f)(name, $value.to_string());
    };
}

impl<F> Visit for StringVisitor<F>
where
    F: FnMut(&'static str, String),
{
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        record_map_value!(self, field, value);
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        record_map_value!(self, field, value);
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        record_map_value!(self, field, format!("{value:?}"));
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        record_map_value!(self, field, value);
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        record_map_value!(self, field, value);
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        record_map_value!(self, field, value);
    }

    fn record_i128(&mut self, field: &tracing::field::Field, value: i128) {
        record_map_value!(self, field, value);
    }

    fn record_u128(&mut self, field: &tracing::field::Field, value: u128) {
        record_map_value!(self, field, value);
    }
}

struct ChainVisitor<A: Visit, B: Visit>(A, B);

impl<A, B> Visit for ChainVisitor<A, B>
where
    A: Visit,
    B: Visit,
{
    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        self.0.record_f64(field, value);
        self.1.record_f64(field, value);
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        self.0.record_i64(field, value);
        self.1.record_i64(field, value);
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        self.0.record_u64(field, value);
        self.1.record_u64(field, value);
    }

    fn record_i128(&mut self, field: &tracing::field::Field, value: i128) {
        self.0.record_i128(field, value);
        self.1.record_i128(field, value);
    }

    fn record_u128(&mut self, field: &tracing::field::Field, value: u128) {
        self.0.record_u128(field, value);
        self.1.record_u128(field, value);
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        self.0.record_bool(field, value);
        self.1.record_bool(field, value);
    }

    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        self.0.record_str(field, value);
        self.1.record_str(field, value);
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        self.0.record_debug(field, value);
        self.1.record_debug(field, value);
    }

    fn record_error(
        &mut self,
        field: &tracing::field::Field,
        value: &(dyn std::error::Error + 'static),
    ) {
        self.0.record_error(field, value);
        self.1.record_error(field, value);
    }
}

pub(crate) trait VisitExt {
    fn chain(self, other: impl Visit) -> impl Visit;
}

impl<T> VisitExt for T
where
    T: Visit,
{
    fn chain(self, other: impl Visit) -> impl Visit {
        ChainVisitor(self, other)
    }
}
