use foyer::{Compression, DirectFsDeviceOptions, Engine, HybridCache, HybridCacheBuilder};
use http_cache_reqwest::{CacheManager, HttpResponse};
use http_cache_semantics::CachePolicy;
use std::error::Error;

pub(crate) struct FoyerCache {
    cache: HybridCache<String, Vec<u8>>,
}

impl FoyerCache {
    pub(crate) async fn new(dir: &str) -> anyhow::Result<Self> {
        let cache = HybridCacheBuilder::<String, Vec<u8>>::new()
            .memory(64 * 1024)
            .with_weighter(|key, value| key.len() + value.len())
            .storage(Engine::Large)
            .with_device_options(
                DirectFsDeviceOptions::new(std::path::Path::new(dir))
                    .with_capacity(1024 * 1024 * 256),
            )
            .with_compression(Compression::Zstd)
            .build()
            .await?;
        Ok(Self { cache })
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
struct Store {
    response: HttpResponse,
    policy: CachePolicy,
}

#[tonic::async_trait]
impl CacheManager for FoyerCache {
    async fn get(
        &self,
        cache_key: &str,
    ) -> Result<Option<(HttpResponse, CachePolicy)>, Box<dyn Error + Sync + Send>> {
        match self.cache.get(&String::from(cache_key)).await? {
            Some(e) => {
                let store: Store = cbor4ii::serde::from_slice(e.value())?;
                Ok(Some((store.response, store.policy)))
            }
            None => Ok(None),
        }
    }

    async fn put(
        &self,
        cache_key: String,
        response: HttpResponse,
        policy: CachePolicy,
    ) -> Result<HttpResponse, Box<dyn Error + Sync + Send>> {
        let store = Store {
            response: response.clone(),
            policy,
        };
        self.cache
            .insert(cache_key, cbor4ii::serde::to_vec(vec![], &store)?);
        Ok(response)
    }

    async fn delete(&self, cache_key: &str) -> Result<(), Box<dyn Error + Sync + Send>> {
        self.cache.remove(cache_key);
        Ok(())
    }
}
