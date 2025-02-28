use tracing::Span;

use crate::TraceRequest;

pub struct RequestOptions<C = ()>
where
    C: RequestContext,
{
    pub(crate) context: C,
}

impl Default for RequestOptions<()> {
    fn default() -> Self {
        Self { context: () }
    }
}

impl<C: RequestContext> RequestOptions<C> {
    pub fn new(c: C) -> Self {
        Self { context: c }
    }
}

pub trait RequestContext {
    fn make_span(&mut self, _: &TraceRequest) -> Span;
}

impl RequestContext for () {
    fn make_span(&mut self, _: &TraceRequest) -> Span {
        Span::none()
    }
}
