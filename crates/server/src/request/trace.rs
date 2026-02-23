pub enum TraceRequest<'a> {
    Http(&'a http::request::Parts),
}
