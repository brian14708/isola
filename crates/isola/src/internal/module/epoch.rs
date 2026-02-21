use std::{
    collections::HashMap,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

use parking_lot::Mutex;
use wasmtime::Engine;

const EPOCH_TICK: Duration = Duration::from_millis(10);

/// Shared global epoch ticker state.
struct EpochTickerShared {
    engines: Mutex<HashMap<u64, Engine>>,
    next_id: AtomicU64,
}

pub struct GlobalEpochTicker {
    shared: Arc<EpochTickerShared>,
}

/// Registration that keeps epoch ticks active for a specific engine.
pub struct EpochTickerRegistration {
    id: u64,
    shared: Arc<EpochTickerShared>,
}

impl GlobalEpochTicker {
    fn new() -> std::io::Result<Self> {
        let shared = Arc::new(EpochTickerShared {
            engines: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        });

        let shared_bg = Arc::clone(&shared);
        std::thread::Builder::new()
            .name("isola-epoch-ticker".to_string())
            .spawn(move || {
                // Keep epoch progression independent of Tokio scheduling.
                // This avoids timeout starvation in current-thread runtimes.
                loop {
                    std::thread::park_timeout(EPOCH_TICK);
                    let engines: Vec<Engine> = shared_bg.engines.lock().values().cloned().collect();
                    for engine in engines {
                        engine.increment_epoch();
                    }
                }
            })?;

        Ok(Self { shared })
    }

    pub fn register(&self, engine: Engine) -> Arc<EpochTickerRegistration> {
        let id = self.shared.next_id.fetch_add(1, Ordering::Relaxed);
        let mut engines = self.shared.engines.lock();
        engines.insert(id, engine);
        drop(engines);

        Arc::new(EpochTickerRegistration {
            id,
            shared: Arc::clone(&self.shared),
        })
    }
}

impl Drop for EpochTickerRegistration {
    fn drop(&mut self) {
        let mut engines = self.shared.engines.lock();
        engines.remove(&self.id);
    }
}

pub fn global_epoch_ticker() -> std::io::Result<&'static GlobalEpochTicker> {
    static GLOBAL_EPOCH_TICKER: OnceLock<
        core::result::Result<GlobalEpochTicker, (std::io::ErrorKind, String)>,
    > = OnceLock::new();

    let ticker = GLOBAL_EPOCH_TICKER
        .get_or_init(|| GlobalEpochTicker::new().map_err(|e| (e.kind(), e.to_string())));
    match ticker {
        Ok(ticker) => Ok(ticker),
        Err((kind, message)) => Err(std::io::Error::new(*kind, message.clone())),
    }
}
