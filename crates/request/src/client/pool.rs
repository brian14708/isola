use std::sync::{
    Arc,
    atomic::{AtomicU32, Ordering},
};

use dashmap::DashMap;
use fastant::{Atomic, Instant};

use super::config::RequestConfig;
use crate::Error;

pub struct ClientToken {
    pub client: reqwest::Client,
    num_inflight: Arc<AtomicU32>,
}

struct ClientEntry {
    client: reqwest::Client,
    num_inflight: Arc<AtomicU32>,
    last_used: Atomic,
}

#[derive(Clone)]
pub struct ClientPool {
    buckets: Arc<DashMap<RequestConfig, Vec<ClientEntry>>>,
    max_inflight: u32,
}

impl ClientEntry {
    fn new(client: reqwest::Client) -> (Self, ClientToken) {
        let c = Self {
            client,
            num_inflight: Arc::new(AtomicU32::new(1)), // Start with 1 for the token we're creating
            last_used: Atomic::new(Instant::now()),
        };
        let token = c.clone_token();
        (c, token)
    }

    fn try_reserve(&self, max_inflight: u32) -> Option<ClientToken> {
        // Try to atomically increment ref count if under limit
        let current =
            self.num_inflight
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |count| {
                    if count >= max_inflight {
                        None // Max inflight reached
                    } else {
                        Some(count + 1)
                    }
                });

        match current {
            Ok(_) => {
                self.last_used.store(Instant::now(), Ordering::Relaxed);
                Some(self.clone_token())
            }
            Err(_) => None, // Max inflight reached
        }
    }

    fn clone_token(&self) -> ClientToken {
        ClientToken {
            client: self.client.clone(),
            num_inflight: Arc::clone(&self.num_inflight),
        }
    }
}

impl ClientPool {
    pub fn new(max_inflight: u32) -> Self {
        Self {
            buckets: Arc::new(DashMap::new()),
            max_inflight,
        }
    }

    pub fn reserve(&self, config: RequestConfig) -> Result<ClientToken, Error> {
        // Try to find an existing client in the bucket for this config
        if let Some(bucket) = self.buckets.get(&config) {
            for entry in bucket.iter() {
                if let Some(token) = entry.try_reserve(self.max_inflight) {
                    return Ok(token);
                }
            }
        }

        let (client, token) = ClientEntry::new(config.build_client()?);

        let mut bucket = self.buckets.entry(config).or_default();
        if let Some(entry) = bucket.iter_mut().last() {
            // If we found an existing entry, try to reserve it
            if let Some(token) = entry.try_reserve(self.max_inflight) {
                return Ok(token);
            }
        }
        bucket.push(client);
        Ok(token)
    }

    pub fn cleanup(&self, max_age: std::time::Duration) {
        let now = Instant::now();

        // Clean up each bucket
        self.buckets.retain(|_, bucket| {
            bucket.retain(|entry| {
                now.duration_since(entry.last_used.load(Ordering::Relaxed)) < max_age
                    || entry.num_inflight.load(Ordering::Relaxed) > 0
            });
            !bucket.is_empty() // Remove empty buckets
        });
    }
}

impl Drop for ClientToken {
    fn drop(&mut self) {
        // Decrement the reference count when the token is dropped
        let prev = self.num_inflight.fetch_sub(1, Ordering::Relaxed);
        debug_assert!(prev > 0, "ClientToken underflow: count was already zero");
    }
}
