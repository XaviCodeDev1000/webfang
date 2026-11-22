//! Construction path for the crawl session run owner.
//!
//! [`CrawlSessionBuilder`] is the only construction site (D1: one place per
//! run setting; adding a knob means adding one builder field). Re-exported
//! from [`crate::application::crawler::session`] so consumer paths stay
//! unchanged.

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::domain::persistence::PersistenceMode;
use crate::domain::{CrawlerConfig, DiscoveredUrl};

use super::{
    CrawlIdentity, CrawlPorts, CrawlSession, CrawlSessionError, PersistencePolicy, TransportPolicy,
};

/// Builder for [`CrawlSession`] — the only construction site (D1: one place
/// per run setting; adding a knob means adding one builder field).
#[derive(Default)]
pub(crate) struct CrawlSessionBuilder {
    config: Option<CrawlerConfig>,
    persistence: Option<PersistenceMode>,
    transport: Option<TransportPolicy>,
    ports: Option<CrawlPorts>,
    identity: Option<CrawlIdentity>,
    /// Wider than the sibling fields: the re-parented session tests assert
    /// the carried seeds directly (`pub(super)` = session and descendants).
    pub(super) extra_seeds: Vec<DiscoveredUrl>,
}

impl CrawlSessionBuilder {
    /// Target configuration (required).
    pub(crate) fn config(mut self, config: CrawlerConfig) -> Self {
        self.config = Some(config);
        self
    }

    /// Persistence input (required; D4: the resolved mode, nothing else).
    pub(crate) fn persistence(mut self, mode: PersistenceMode) -> Self {
        self.persistence = Some(mode);
        self
    }

    /// Fetch policy (required).
    pub(crate) fn transport(mut self, policy: TransportPolicy) -> Self {
        self.transport = Some(policy);
        self
    }

    /// Injected seams (required; `None` members mean historical defaults).
    pub(crate) fn ports(mut self, ports: CrawlPorts) -> Self {
        self.ports = Some(ports);
        self
    }

    /// Run identity (required; validated by [`build`](Self::build)).
    ///
    /// #1439: the builder no longer mints a fallback root — every entry that
    /// reaches `build()` must carry the operation's run-root explicitly, so a
    /// session can never silently split a run into two identities. A
    /// standalone entry that IS the operation mints its own root at ITS
    /// boundary (the documented contract of the deprecated `crawl_site`
    /// shims), never the builder on their behalf.
    pub(crate) fn identity(mut self, identity: CrawlIdentity) -> Self {
        self.identity = Some(identity);
        self
    }

    /// Sitemap-discovered URLs to inject as additional seeds (optional;
    /// empty by default). Carried into the session and drained by
    /// [`Engine::run`](crate::application::crawler::engine::Engine::run) via
    /// `push_prioritized` with [`UrlSource::Sitemap`](crate::domain::crawler_port::UrlSource),
    /// so enqueue-time dedup absorbs seed/sitemap overlap.
    pub(crate) fn extra_seeds(mut self, seeds: Vec<DiscoveredUrl>) -> Self {
        self.extra_seeds = seeds;
        self
    }

    /// Validate the assembled description completely before any worker spawns.
    ///
    /// # Errors
    ///
    /// Returns [`CrawlSessionError::InvalidConfiguration`] when a required
    /// part (including the run identity, #1439) is missing, the seed scheme
    /// is not fetchable, the backoff bounds are inverted, or a checkpoint
    /// target carries a zero interval.
    pub(crate) fn build(self) -> Result<CrawlSession, CrawlSessionError> {
        let config = self.config.ok_or_else(|| {
            CrawlSessionError::InvalidConfiguration("missing target configuration".to_string())
        })?;
        if !matches!(config.seed_url.scheme(), "http" | "https") {
            return Err(CrawlSessionError::InvalidConfiguration(format!(
                "seed scheme '{}' is not fetchable (http/https required)",
                config.seed_url.scheme()
            )));
        }
        let persistence = self.persistence.ok_or_else(|| {
            CrawlSessionError::InvalidConfiguration("missing persistence policy".to_string())
        })?;
        if let Some(interval) = checkpoint_interval_of(&persistence) {
            if interval == 0 {
                return Err(CrawlSessionError::InvalidConfiguration(
                    "checkpoint target set with zero interval".to_string(),
                ));
            }
        }
        let transport = self.transport.ok_or_else(|| {
            CrawlSessionError::InvalidConfiguration("missing transport policy".to_string())
        })?;
        if transport.backoff_max_ms < transport.backoff_base_ms {
            return Err(CrawlSessionError::InvalidConfiguration(format!(
                "backoff_max_ms ({}) < backoff_base_ms ({})",
                transport.backoff_max_ms, transport.backoff_base_ms
            )));
        }
        let ports = self.ports.ok_or_else(|| {
            CrawlSessionError::InvalidConfiguration("missing port bundle".to_string())
        })?;
        // #1439 run-root propagation: identity is REQUIRED, not defaulted.
        // The old `unwrap_or_else(CorrelationId::new)` fallback let any
        // forgotten `.identity(...)` silently split a run into two
        // causability roots (the #687 class, re-broken by the #1232 rework).
        // Missing identity now fails exactly like a missing port bundle —
        // before any worker spawns.
        let identity = self.identity.ok_or_else(|| {
            CrawlSessionError::InvalidConfiguration("missing run identity".to_string())
        })?;
        Ok(CrawlSession {
            identity,
            config: Arc::new(config),
            persistence: PersistencePolicy {
                mode: persistence,
                loaded: None,
            },
            transport,
            ports,
            extra_seeds: self.extra_seeds,
            cancel_token: CancellationToken::new(),
        })
    }
}

/// Checkpoint interval carried by a mode (`None` = persistence off).
fn checkpoint_interval_of(mode: &PersistenceMode) -> Option<u64> {
    match mode {
        PersistenceMode::Disabled | PersistenceMode::Resume { .. } => None,
        PersistenceMode::Checkpoint { cfg }
        | PersistenceMode::Full {
            checkpoint: cfg, ..
        } => Some(cfg.interval),
    }
}
