use tracing::field::Visit;

pub struct FieldVisitor<Pred: Fn(&'static str) -> bool, Rec: FnMut(&'static str, String)> {
    pred: Pred,
    rec: Rec,
}

impl<Pred, Rec> FieldVisitor<Pred, Rec>
where
    Pred: Fn(&'static str) -> bool,
    Rec: FnMut(&'static str, String),
{
    pub fn new(pred: Pred, rec: Rec) -> Self {
        Self { pred, rec }
    }
}

macro_rules! filter_map_value {
    ($self:ident, $field:expr, $value:expr) => {
        let name = $field.name();
        if !($self.pred)(name) {
            return;
        }
        ($self.rec)(name, $value.to_string());
    };
}

impl<Pred, Rec> Visit for FieldVisitor<Pred, Rec>
where
    Pred: Fn(&'static str) -> bool,
    Rec: FnMut(&'static str, String),
{
    fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
        filter_map_value!(self, field, value);
    }

    fn record_bool(&mut self, field: &tracing::field::Field, value: bool) {
        filter_map_value!(self, field, value);
    }

    fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
        filter_map_value!(self, field, format!("{value:?}"));
    }

    fn record_f64(&mut self, field: &tracing::field::Field, value: f64) {
        filter_map_value!(self, field, value);
    }

    fn record_i64(&mut self, field: &tracing::field::Field, value: i64) {
        filter_map_value!(self, field, value);
    }

    fn record_u64(&mut self, field: &tracing::field::Field, value: u64) {
        filter_map_value!(self, field, value);
    }

    fn record_i128(&mut self, field: &tracing::field::Field, value: i128) {
        filter_map_value!(self, field, value);
    }

    fn record_u128(&mut self, field: &tracing::field::Field, value: u128) {
        filter_map_value!(self, field, value);
    }
}
