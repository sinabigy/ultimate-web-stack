use std::{
    ops::Deref,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use app_config::AppConfig;
use app_messaging::EventBus;
use app_rate_limit::RateLimiter;
use app_telemetry::MetricsHandle;
use tokio::sync::Semaphore;
use tokio_util::sync::CancellationToken;

use crate::health::HealthCheck;

#[derive(Debug, Clone, serde::Serialize, ts_rs::TS)]
#[ts(export)]
pub struct BuildInfo {
    pub name: &'static str,
    pub version: &'static str,
    pub git_sha: &'static str,
    pub profile: &'static str,
}

/// Lifecycle flags shared by the server loop and handlers.
#[derive(Default)]
pub struct Lifecycle {
    draining: AtomicBool,
    /// Cancelled when the server stops; long-lived streams (SSE/WS) end on it.
    pub shutdown: CancellationToken,
}

impl Lifecycle {
    pub fn start_draining(&self) {
        self.draining.store(true, Ordering::SeqCst);
    }
    pub fn is_draining(&self) -> bool {
        self.draining.load(Ordering::SeqCst)
    }
}

/// Shared state. Cloning is a pointer copy.
#[derive(Clone)]
pub struct AppState(Arc<AppInner>);

pub struct AppInner {
    pub config: AppConfig,
    pub build: BuildInfo,
    pub metrics: Option<MetricsHandle>,
    pub health: Vec<Arc<dyn HealthCheck>>,
    pub events: Arc<dyn EventBus>,
    pub rate_limiter: Option<Arc<dyn RateLimiter>>,
    pub lifecycle: Lifecycle,
    /// Global in-flight cap for ordinary requests (load shedding).
    pub inflight: Arc<Semaphore>,
    /// Database, identity and authorization services. `None` only in minimal tests.
    pub services: Option<Arc<crate::services::Services>>,
    /// Live outbound-provider health (set when the provider engine is running).
    pub provider_health: Option<ProviderHealthFn>,
    /// Response/aggregate cache (in-memory by default; Redis/Dragonfly in the performance profile).
    pub cache: app_cache::CacheLayer,
    /// Analytics events (never blocks; no-op when analytics is disabled).
    pub analytics: Arc<dyn app_analytics::AnalyticsSink>,
    /// Tenant-scoped analytics reads (ClickHouse), when enabled.
    pub analytics_query: Option<Arc<app_analytics::AnalyticsQuery>>,
}

pub type ProviderHealthFn = Arc<dyn Fn(&str) -> Option<crate::dto::ProviderHealth> + Send + Sync>;

impl AppInner {
    /// Services or 503 (never a panic) when the instance was built without them.
    pub fn svc(&self) -> Result<&Arc<crate::services::Services>, app_errors::ApiError> {
        self.services.as_ref().ok_or(app_errors::ApiError::Unavailable("application services"))
    }
}

impl Deref for AppState {
    type Target = AppInner;
    fn deref(&self) -> &AppInner {
        &self.0
    }
}

pub struct AppStateBuilder {
    config: AppConfig,
    build: BuildInfo,
    metrics: Option<MetricsHandle>,
    health: Vec<Arc<dyn HealthCheck>>,
    events: Option<Arc<dyn EventBus>>,
    rate_limiter: Option<Arc<dyn RateLimiter>>,
    services: Option<Arc<crate::services::Services>>,
    provider_health: Option<ProviderHealthFn>,
    cache: Option<app_cache::CacheLayer>,
    analytics: Option<Arc<dyn app_analytics::AnalyticsSink>>,
    analytics_query: Option<Arc<app_analytics::AnalyticsQuery>>,
}

impl AppState {
    pub fn builder(config: AppConfig, build: BuildInfo) -> AppStateBuilder {
        AppStateBuilder {
            config,
            build,
            metrics: None,
            health: Vec::new(),
            events: None,
            rate_limiter: None,
            services: None,
            provider_health: None,
            cache: None,
            analytics: None,
            analytics_query: None,
        }
    }
}

impl AppStateBuilder {
    pub fn metrics(mut self, h: MetricsHandle) -> Self {
        self.metrics = Some(h);
        self
    }
    pub fn health_check(mut self, c: Arc<dyn HealthCheck>) -> Self {
        self.health.push(c);
        self
    }
    pub fn events(mut self, bus: Arc<dyn EventBus>) -> Self {
        self.events = Some(bus);
        self
    }
    pub fn rate_limiter(mut self, l: Arc<dyn RateLimiter>) -> Self {
        self.rate_limiter = Some(l);
        self
    }
    pub fn services(mut self, s: Arc<crate::services::Services>) -> Self {
        self.services = Some(s);
        self
    }
    pub fn cache(mut self, c: app_cache::CacheLayer) -> Self {
        self.cache = Some(c);
        self
    }
    pub fn analytics(mut self, s: Arc<dyn app_analytics::AnalyticsSink>) -> Self {
        self.analytics = Some(s);
        self
    }
    pub fn analytics_query(mut self, q: Arc<app_analytics::AnalyticsQuery>) -> Self {
        self.analytics_query = Some(q);
        self
    }
    pub fn provider_health(mut self, f: ProviderHealthFn) -> Self {
        self.provider_health = Some(f);
        self
    }
    pub fn build(self) -> AppState {
        let inflight = Arc::new(Semaphore::new(self.config.http.max_inflight));
        let cache = self.cache.unwrap_or_else(|| {
            app_cache::CacheLayer::new(
                Arc::new(app_cache::memory::MemoryCache::new(self.config.cache.memory_max_entries)),
                &self.config.cache.namespace,
            )
        });
        AppState(Arc::new(AppInner {
            events: self.events.unwrap_or_else(|| Arc::new(app_messaging::LocalEventBus::default())),
            config: self.config,
            build: self.build,
            metrics: self.metrics,
            health: self.health,
            rate_limiter: self.rate_limiter,
            lifecycle: Lifecycle::default(),
            inflight,
            cache,
            services: self.services,
            provider_health: self.provider_health,
            analytics: self.analytics.unwrap_or_else(|| Arc::new(app_analytics::NoopSink)),
            analytics_query: self.analytics_query,
        }))
    }
}
