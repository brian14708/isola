mod client;
mod http;
mod options;
mod trace;

pub type Error = Box<dyn std::error::Error + Send + Sync>;
pub type WebsocketMessage = tokio_tungstenite::tungstenite::Message;

pub use client::Client;
pub use options::{RequestContext, RequestOptions};
pub use trace::TraceRequest;
