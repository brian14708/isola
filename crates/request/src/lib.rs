mod client;
mod error;
mod http;
mod options;
mod trace;

pub use error::Error;

pub type WebsocketMessage = tokio_tungstenite::tungstenite::Message;

pub use client::{Client, ClientBuilder};
pub use options::{RequestContext, RequestOptions};
pub use trace::TraceRequest;
