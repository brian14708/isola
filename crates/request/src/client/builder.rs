use super::Client;

#[must_use]
pub struct ClientBuilder {
    pub(crate) max_inflight_per_client: u32,
    pub(crate) client_idle_timeout: std::time::Duration,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self {
            max_inflight_per_client: 96,
            client_idle_timeout: std::time::Duration::from_mins(1),
        }
    }
}

impl ClientBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    pub const fn max_inflight_per_client(mut self, max_inflight_per_client: u32) -> Self {
        self.max_inflight_per_client = max_inflight_per_client;
        self
    }

    #[must_use]
    pub fn build(self) -> Client {
        Client::build(&self)
    }
}
