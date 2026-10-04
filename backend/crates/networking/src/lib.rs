//! Outbound API engine.
//!
//! A [`ProviderRegistry`] holds one [`Provider`] per external API (OpenAI, Anthropic, Shopify,
//! Stripe, any REST API: nothing here is vendor-specific). Each provider owns:
//!
//! ```text
//! call ─► deadline ─► circuit breaker ─► rate limiter (req/s) ─► token budget (tokens/min)
//!      ─► adaptive concurrency permit (priority queue, bounded) ─► pooled HTTP client
//!      ─► classify ─► feedback to limiter/breaker/stats ─► retry (full jitter, budget, Retry-After)
//! ```
//!
//! The objective is **successful useful throughput**, not raw request rate. Two controllers:
//! - *concurrency* (AIMD): 503/504/timeouts or sustained latency growth shrink the limit
//!   multiplicatively; healthy, fully used capacity grows it additively;
//! - *request rate*: learned from 429s (ceiling memory, hold just below it, probe slowly);
//!   a 429 with `Retry-After` pauses the whole provider.
//!
//! See docs/architecture/outbound-engine.md for the algorithm and tuning.

pub mod adaptive;
pub mod breaker;
pub mod provider;
pub mod rate;
pub mod retry;
pub mod stats;

use std::{collections::BTreeMap, sync::Arc};

pub use adaptive::{AdaptiveConfig, AdaptiveLimiter, Outcome};
pub use provider::{CallError, CallRequest, CallResponse, Priority, Provider, ProviderSettings};
pub use stats::HealthSnapshot;

/// All configured providers, built once at startup and shared.
#[derive(Clone, Default)]
pub struct ProviderRegistry {
    providers: Arc<BTreeMap<String, Arc<Provider>>>,
}

impl ProviderRegistry {
    pub fn from_config(cfg: &app_config::ProvidersConfig) -> Result<Self, provider::BuildError> {
        let mut map = BTreeMap::new();
        for (name, def) in &cfg.definitions {
            map.insert(name.clone(), Arc::new(Provider::new(name, ProviderSettings::from_definition(def))?));
        }
        Ok(Self { providers: Arc::new(map) })
    }

    pub fn with(providers: Vec<Arc<Provider>>) -> Self {
        Self { providers: Arc::new(providers.into_iter().map(|p| (p.name().to_string(), p)).collect()) }
    }

    pub fn get(&self, name: &str) -> Option<&Arc<Provider>> {
        self.providers.get(name)
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.providers.keys().map(String::as_str)
    }

    pub fn health(&self, name: &str) -> Option<HealthSnapshot> {
        self.providers.get(name).map(|p| p.health())
    }
}
