//! Three-layer hybrid downloader with SPA-aware escalation.
//!
//! Fetch strategy:
//!
//! 1. **Layer 1 — wreq**: fast static HTTP fetch
//! 2. **SPA detection**: analyse the HTML for SPA mount points / WAF markers
//! 3. **Layer 2 — Obscura**: subprocess HTML render, its output re-classified
//!    by the same SPA/WAF verdict (if SPA detected)
//! 4. **Layer 3 — Chromiumoxide**: full CDP rendering (if Obscura insufficient)
//!
//! WAF detection at **any** layer short-circuits with an error — escalation is
//! never attempted against a WAF challenge.
//!
//! Uses generics over the `Downloader` trait because native `async fn` in traits
//! is not dyn-compatible. Each layer is a separate type parameter.

use tracing::{debug, instrument, warn};
use url::Url;

use futures::future::BoxFuture;
use tokio_util::sync::CancellationToken;

use crate::domain::waf::InspectionContext;

use super::resource_governor::ResourceGovernor;
use super::spa_detector::{detect_spa, SpaSignal};
use super::{DownloadError, Downloader, FetchedPage};

/// Three-layer hybrid downloader.
///
/// Type parameters correspond to the three fetch layers:
/// - `L1`: static HTTP (typically [`WreqDownloader`](super::wreq_downloader::WreqDownloader))
/// - `L2`: subprocess fallback (typically [`ObscuraDownloader`](super::obscura_downloader::ObscuraDownloader))
/// - `L3`: headless browser (typically [`ChromiumoxideDownloader`](super::chromiumoxide_downloader::ChromiumoxideDownloader))
///
/// # SSRF residual (F-R3-7 — accepted by design, ADR-0016 §6)
///
/// L2 and L3 perform their own networking; the L1 dial-level SSRF guard
/// ([`crate::domain::ssrf_guard`]) does not run inside those processes. An
/// entry-time re-check does not close the resulting DNS-rebinding TOCTOU
/// window (it re-opens between the re-check and the subprocess's own dial),
/// so the residual is LOW and ACCEPTED — see ADR-0016 §6. Re-evaluate this
/// acceptance whenever L2/L3 networking changes.
pub struct HybridRouter<L1: Downloader, L2: Downloader, L3: Downloader> {
    layer1: L1,
    layer2: L2,
    layer3: L3,
    governor: ResourceGovernor,
    /// Bypass WAF classification on the spa-detection path (REQ-WAF-07, W1).
    ///
    /// When `true`, a genuine T1 challenge is treated per normal spa/static
    /// logic instead of aborting with [`DownloadError::WafChallenge`].
    ignore_waf: bool,
}

impl<L1: Downloader, L2: Downloader, L3: Downloader> HybridRouter<L1, L2, L3> {
    /// Build a hybrid router whose internal [`ResourceGovernor`] shares the
    /// caller's cancellation token (issue #1009, mirrors #509 for the Full
    /// strategy).
    ///
    /// Permit waits inside the L2/L3 escalation abort with
    /// [`DownloadError::Cancelled`] when the token fires; pass an inert
    /// [`CancellationToken::new`] where no shutdown policy exists.
    pub fn new(
        layer1: L1,
        layer2: L2,
        layer3: L3,
        ignore_waf: bool,
        cancel_token: CancellationToken,
    ) -> Self {
        Self {
            layer1,
            layer2,
            layer3,
            governor: ResourceGovernor::with_cancel_token(cancel_token),
            ignore_waf,
        }
    }

    /// Build a hybrid router around an explicit governor.
    ///
    /// Calibration seam behind [`new`](Self::new), mirroring
    /// [`ResourceGovernor::with_max_instances`](super::resource_governor::ResourceGovernor::with_max_instances):
    /// lets a test drive the admission boundary (permits exhausted, or a
    /// warning-level reduction to zero) without fabricating host RAM pressure.
    #[cfg(test)]
    pub(crate) fn with_governor(
        layer1: L1,
        layer2: L2,
        layer3: L3,
        ignore_waf: bool,
        governor: ResourceGovernor,
    ) -> Self {
        Self {
            layer1,
            layer2,
            layer3,
            governor,
            ignore_waf,
        }
    }

    /// Refuse an escalation the governor did not grant a permit for (S17).
    ///
    /// `check_resources` answers how many permits this layer could take right
    /// now; `Ok(0)` is a denial exactly like the error arms — the warning-level
    /// halving exists to stop escalation under pressure, not to be ignored by
    /// the last caller that only matched `Err`.
    fn check_escalation_budget(&self, url: &Url, layer: u8) -> Result<(), DownloadError> {
        match self.governor.check_resources() {
            Ok(permits) if permits > 0 => Ok(()),
            Ok(_) => {
                warn!(
                    url = %url,
                    layer,
                    "ResourceGovernor denied escalation layer: no permits available"
                );
                Err(DownloadError::from(
                    super::resource_governor::ResourceError::Insufficient(
                        "no escalation permits available".to_string(),
                    ),
                ))
            },
            Err(e) => {
                warn!(url = %url, layer, error = %e, "ResourceGovernor denied escalation layer");
                Err(DownloadError::from(e))
            },
        }
    }

    /// Build the inspection context from the real response instead of the
    /// degraded context the spa detector used to synthesize internally.
    ///
    /// Status, headers and content-type come straight from the fetched page;
    /// the body sample is `page.html` (passed to `detect_spa` separately) and
    /// the response cookies are carried as a synthetic `cookie` header so the
    /// full response context reaches classification. `ignore_waf` is the
    /// caller-configurable policy held by this router (REQ-WAF-07).
    fn evaluation_context(&self, page: &FetchedPage) -> InspectionContext {
        let mut headers = page.headers.clone();
        if !page.cookies.is_empty() {
            let cookie_header = page
                .cookies
                .iter()
                .map(|cookie| format!("{}={}", cookie.name, cookie.value))
                .collect::<Vec<_>>()
                .join("; ");
            headers.insert("cookie".to_string(), cookie_header);
        }
        InspectionContext {
            status: Some(page.status),
            content_type: headers.get("content-type").cloned(),
            headers,
            ignore_waf: self.ignore_waf,
        }
    }

    /// Inspect a [`FetchedPage`] and decide whether escalation is needed.
    ///
    /// Returns `Ok(true)` when the page has usable static content (the caller
    /// returns it), `Ok(false)` when SPA detection asks for the next layer,
    /// and `Err(DownloadError)` for a WAF challenge (which aborts escalation).
    fn evaluate_fetch(&self, page: &FetchedPage) -> Result<bool, DownloadError> {
        let ctx = self.evaluation_context(page);
        let signal = detect_spa(&page.html, &ctx);

        match signal {
            SpaSignal::StaticContent => {
                debug!("SPA check: static content — no escalation needed");
                Ok(true)
            },
            SpaSignal::WafBlocked => {
                warn!("WAF detected at fetch time — aborting escalation");
                Err(DownloadError::WafChallenge(
                    "WAF challenge detected in response".to_string(),
                ))
            },
            SpaSignal::SpaDetected(reason) => {
                debug!("SPA detected ({reason:?}) — escalation warranted");
                Ok(false)
            },
        }
    }
}

impl<L1: Downloader, L2: Downloader, L3: Downloader> HybridRouter<L1, L2, L3> {
    #[instrument(skip(self), fields(url = %url))]
    async fn fetch_inner(&self, url: &Url) -> Result<FetchedPage, DownloadError> {
        // --- Layer 1: fast static HTTP ---
        debug!("Layer 1 (wreq): fetching {url}");
        let page = match self.layer1.fetch(url).await {
            Ok(p) => p,
            Err(DownloadError::WafChallenge(msg)) => {
                return Err(DownloadError::WafChallenge(msg));
            },
            Err(e) => {
                debug!("Layer 1 failed ({e}) — aborting");
                return Err(e);
            },
        };

        // SPA detected — continue escalation; static content — return early
        if self.evaluate_fetch(&page)? {
            return Ok(page);
        }

        // --- Layer 2: Obscura subprocess ---
        debug!("Layer 2 (Obscura): attempting fetch for {url}");

        // Check resources before spawning a subprocess
        self.check_escalation_budget(url, 2)?;

        match self.layer2.fetch(url).await {
            Ok(page) if !page.html.is_empty() => {
                // S5 (#2722 H3): the L2 body is classified exactly like L1's —
                // a challenge rendered through obscura aborts the escalation
                // (REQ-WAF-07), a still-SPA body escalates to L3, and static
                // content is accepted. Previously any non-empty body was
                // accepted unclassified.
                if self.evaluate_fetch(&page)? {
                    debug!(
                        url = %url,
                        layer = 2,
                        final_url = %page.url,
                        status = page.status,
                        "Layer 2 returned {} bytes",
                        page.html.len()
                    );
                    return Ok(page);
                }
                debug!("Layer 2 body still looks like a SPA — will try Layer 3");
            },
            Ok(_) => {
                debug!("Layer 2 returned empty content — will try Layer 3");
            },
            Err(DownloadError::WafChallenge(msg)) => {
                return Err(DownloadError::WafChallenge(msg));
            },
            Err(e) => {
                debug!("Layer 2 failed ({e}) — will try Layer 3");
            },
        }

        // --- Layer 3: Chromiumoxide CDP ---
        debug!("Layer 3 (Chromiumoxide): attempting fetch for {url}");

        self.check_escalation_budget(url, 3)?;

        match self.layer3.fetch(url).await {
            Ok(page) => {
                debug!("Layer 3 returned {} bytes", page.html.len());
                // S11: classify the terminal layer too. A challenge rendered by
                // Chromiumoxide must abort; a still-SPA body cannot escalate
                // further, so it is returned as the best available result.
                self.evaluate_fetch(&page)?;
                return Ok(page);
            },
            Err(DownloadError::WafChallenge(msg)) => {
                return Err(DownloadError::WafChallenge(msg));
            },
            Err(e) => {
                warn!(url = %url, error = %e, "all fetch layers exhausted");
                return Err(e);
            },
        }
    }
}

impl<L1: Downloader, L2: Downloader, L3: Downloader> Downloader for HybridRouter<L1, L2, L3> {
    fn fetch<'a>(&'a self, url: &'a Url) -> BoxFuture<'a, Result<FetchedPage, DownloadError>> {
        Box::pin(self.fetch_inner(url))
    }

    fn supports_interactions(&self) -> bool {
        self.layer3.supports_interactions()
    }

    fn memory_cost(&self) -> usize {
        self.layer1.memory_cost() + self.layer2.memory_cost() + self.layer3.memory_cost()
    }
}

#[cfg(test)]
#[cfg(not(miri))] // ResourceGovernor uses sysinfo (unsupported sysconf under Miri)
mod tests;
