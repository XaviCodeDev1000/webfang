//! Lifecycle, run and shutdown flow of the crawl engine — the signal
//! handler, run cancellation, the run sequence and its drain/summary tail,
//! extracted verbatim from `engine.rs` (file-size Ola 1, #2584). Inherent
//! methods of [`Engine`]: the child module reaches the engine's private
//! fields, and the module stays private — no re-export is needed because
//! nothing here is part of the crate's public surface.

use std::sync::atomic::AtomicUsize;
use std::sync::Arc;
use std::time::Duration;

use tokio_util::sync::CancellationToken;
use tracing::{info, warn, Instrument};

use super::Engine;
use super::{
    PauseWatchdog, ShutdownSignal, TerminationOutcome, FAILED_RUN_DRAIN_GRACE,
    FETCH_TIMEOUT_PHASES, SHUTDOWN_GRACE_SECS, TERMINATION_REGISTRATION_REJECTED,
};
use crate::application::crawler::crawl_task::handle_crawl_result;
use crate::application::crawler::ports::TerminationSource;
use crate::application::crawler::session::{CheckpointAction, CrawlExec};
use crate::application::rate_limiter::SharedRateLimiter;
use crate::domain::crawler_port::UrlSource;
use crate::domain::{CrawlError, CrawlErrorCategory, CrawlResult, CrawlerConfig};

impl Engine {
    /// Spawn a signal handler that sets the shutdown flag on SIGINT/SIGTERM
    /// (and, on unix, SIGHUP — XP-S-03, #1608; on Windows, the console
    /// close/logoff/shutdown events `ctrl_c()` never sees — XP-S-02, #1808).
    ///
    /// WHICH events exist is the injected [`TerminationSource`]'s business,
    /// never this method's: the platform plumbing lives in infrastructure and
    /// is reached through the port. Also fires the cancellation token (#509)
    /// so workers blocked on rate-limit or resource-governor waits abort
    /// instead of hanging. The source only *fires* it: this token stays the
    /// run's one shutdown authority (ADR-0016).
    ///
    /// A REJECTED registration (the OS refused the handler) is not a
    /// termination event: the run keeps going and waits for an explicit
    /// cancel. The decision is the shared one ([`TerminationOutcome`]), the
    /// same one the CLI shutdown guard makes, and both sites say the same
    /// thing ([`TERMINATION_REGISTRATION_REJECTED`]) — #1835.
    // Supervised: the returned JoinHandle is stored by the caller and
    // aborted at shutdown — same lifecycle as `signal_handle` (#1941).
    #[allow(clippy::disallowed_methods)]
    pub(super) fn spawn_signal_handler(
        shutdown: ShutdownSignal,
        cancel: CancellationToken,
        termination_source: TerminationSource,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(
            async move {
                let outcome = TerminationOutcome::from_wait(termination_source.wait().await);
                match outcome {
                    TerminationOutcome::Event(name) => {
                        info!("Received {name} — initiating graceful shutdown");
                    },
                    TerminationOutcome::RegistrationRejected => {
                        // #1835: a rejected registration is NOT a termination
                        // event. The run keeps going and waits for an explicit
                        // cancel — the same answer the CLI shutdown guard
                        // gives, and the same words.
                        warn!(
                            authority = "engine",
                            "{}", TERMINATION_REGISTRATION_REJECTED
                        );
                    },
                }
                if outcome.stops_run() {
                    shutdown.store(true, std::sync::atomic::Ordering::SeqCst);
                    cancel.cancel();
                }
            }
            .in_current_span(),
        )
    }

    /// Fire the run's cancellation authority (#509).
    ///
    /// `from_session` adopts the session's token into the engine field
    /// (P6-2), so both are the same `CancellationToken`; firing the engine
    /// copy keeps the session untouched for its close-path verdicts.
    pub(super) fn cancel_run(&self) {
        self.cancel_token.cancel();
    }

    /// Clone of the engine's cancellation token (#509).
    ///
    /// Fire it to abort workers blocked on rate-limit or resource-governor
    /// waits and unblock [`run`](Self::run)'s drain — the same effect as a
    /// signal-driven shutdown, without an OS signal.
    #[must_use]
    pub fn cancel_handle(&self) -> CancellationToken {
        self.cancel_token.clone()
    }

    /// Adopt a run-scope shared rate limiter (#1428).
    ///
    /// Replaces the per-engine bucket `from_session` minted so `--delay-ms`
    /// paces across the engines of one run instead of granting every engine
    /// a fresh burst. Must be called before [`run`](Self::run) spawns any
    /// task, so no worker ever observes the minted bucket.
    pub(crate) fn set_shared_limiter(&mut self, limiter: SharedRateLimiter) {
        self.rate_limiter = limiter;
    }

    /// Run the crawl loop until completion
    ///
    /// Returns the collected URLs and error count.
    // A run is an ordered sequence of decided steps — seed, loop, drain,
    // verdict, collect, summary — and the #2313 stall propagation spent the
    // last of the line budget it had. Splitting it would hide that order
    // behind jumps, so the length is accepted here rather than paid for there.
    #[allow(clippy::too_many_lines)]
    pub async fn run(&mut self) -> Result<CrawlResult, CrawlError> {
        let config_clone = Arc::clone(&self.config);

        // Spawn signal handler for graceful shutdown
        self.signal_handle = Some(Self::spawn_signal_handler(
            Arc::clone(&self.shutdown),
            self.cancel_token.clone(),
            self.termination_source.clone(),
        ));

        // Load checkpoint state if resuming
        self.restore_checkpoint_state();

        // Seed the scheduler (pushes onto the discovery queue and pending buffer)
        self.scheduler.seed(&config_clone.seed_url).await;

        // Sitemap extra seeds (sitemap-crawl-run-parity D1): drained right
        // after the scheduler seed and before the pattern-filter early
        // return, through the queue port — enqueue-time dedup absorbs
        // seed/sitemap overlap, so the seed page is crawled exactly once.
        // Snapshotted first: no session borrow crosses the enqueue `.await`.
        let extra_seeds = self
            .session
            .as_ref()
            .map(|session| session.extra_seeds().to_vec())
            .unwrap_or_default();
        for discovered in extra_seeds {
            self.scheduler
                .queue()
                .push_prioritized(discovered, UrlSource::Sitemap)
                .await;
        }

        // Apply pattern filtering to seed — #634
        let seed_str = config_clone.seed_url.as_str().to_string();
        if !crate::application::url_filter::is_allowed(&seed_str, &config_clone) {
            info!(
                seed_url = %config_clone.seed_url,
                "Seed URL excluded by pattern filters — exiting with empty result"
            );
            return Ok(CrawlResult::empty());
        }

        let mut tasks = tokio::task::JoinSet::new();

        // Consume the run owner (P6-2 slice 4): the session derives the
        // shared task context here and renders the close verdict at the end
        // of this method. Taking it out makes a second `run()` call fail
        // typed instead of re-running with a context that has no owner.
        let session = match self.session.take() {
            Some(session) => session,
            None => {
                return Err(CrawlError::Internal(
                    "engine run without a crawl session — run() may be called only once per Engine"
                        .to_string(),
                ));
            },
        };

        // Build shared task context once — all spawned tasks share this Arc
        let task_ctx = session.task_ctx(CrawlExec {
            queue: self.scheduler.queue(),
            rate_limiter: self.rate_limiter.clone(),
            pages_crawled: Arc::clone(&self.pages_crawled),
            error_count: Arc::clone(&self.error_count),
            error_breakdown: Arc::clone(&self.error_breakdown),
            collector: self.collector.clone(),
            cookie_bridge: Arc::clone(&self.cookie_bridge),
            banned_domains: Arc::clone(&self.banned_domains),
            robots_fetcher: Arc::clone(&self.robots_fetcher),
            fetch_router: self.fetch_router.clone(),
        });

        // Progress tracking start (issue #356 Fase 4)
        let start = std::time::Instant::now();

        // Bound every worker wait (#509): rate-limit token + both fetch
        // timeout phases + grace; the post-fetch pipeline rides on the grace
        // (residual risk documented on `worker_wait_bound`, #2338).
        let worker_wait_bound = Self::worker_wait_bound(&config_clone);

        // A stall is captured here and reported after the drain.
        let loop_result = self
            .crawl_loop(
                &mut tasks,
                &task_ctx,
                start,
                worker_wait_bound,
                PauseWatchdog::from_config(&config_clone),
            )
            .await;

        // Wait for remaining tasks — bounded, then abort stragglers, so a
        // shutdown can never hang on a blocked worker (#509). A FAILED loop
        // gets only the short grace (#2338): `cancel_run` already fired and
        // the failure reason is known, so the wedged worker must not cost a
        // second full bound before the error surfaces.
        let drain_bound = if loop_result.is_err() {
            FAILED_RUN_DRAIN_GRACE
        } else {
            worker_wait_bound
        };
        Self::drain_tasks(
            &mut tasks,
            &self.error_count,
            &self.error_breakdown,
            drain_bound,
        )
        .await;

        // Drop task_ctx so all cloned Senders inside CrawlTaskCtx are released.
        // Without this, collect() hangs forever — the mpsc channel stays open
        // because a Sender clone inside the Arc<CrawlTaskCtx> is still alive.
        drop(task_ctx);

        // A stalled loop is a FAILED run (#2313); the checkpoint stays for resume.
        // #2338: the failure carries the results the crawl DID collect, so the
        // caller can persist them instead of discarding completed work.
        if let Err(loop_err) = loop_result {
            return Err(self.fail_run_with_partials(loop_err).await);
        }

        // Final checkpoint verdict: a fully-completed crawl (no shutdown, no
        // pending work left, not truncated by max_pages) deletes its
        // checkpoint (F-01) so the next identical run reproduces the same
        // output set instead of resuming stale state. Interrupted or
        // truncated runs keep the file for resume. Checkpoint I/O belongs
        // exclusively to decided verdicts — the periodic and signal saves in
        // the crawl loop, and the close decision executed inside
        // `CrawlSession::finish` (P6-2). `shutdown()` performs no checkpoint
        // I/O, so no handle needs neutralizing here (issue #1823).
        let completed_fully = !self.shutdown.load(std::sync::atomic::Ordering::SeqCst)
            && !self.scheduler.has_pending_work()
            && !self.collector.is_full(self.config.max_pages);
        let close = session
            .finish(completed_fully, async || {
                self.build_checkpoint_state().await
            })
            .await;

        // Collect results via mpsc channel — now all Senders are dropped,
        // so the receiver worker will drain and terminate.
        let summary = self.collect_result_summary().await;
        let total_pages = summary.total_pages;
        let errors = summary.errors;

        // Structured crawl summary (issue #356 Fase 4, error breakdown #374)
        self.log_crawl_summary(total_pages, errors, start);
        // P6-2: the session close carries the run label and the F-01
        // verdict so `--trace-file` shows what happened to resume state.
        // NOTE: distinct message from the canonical `crawl completed`
        // summary above — the benchmark aggregator keys on that message
        // and a second line with the same message but fewer numeric
        // fields breaks its parse (P6-2 slice 1 CI).
        let checkpoint_action = match close.action {
            CheckpointAction::Delete => "deleted",
            CheckpointAction::Write => "wrote",
            CheckpointAction::Skip => "skipped",
        };
        info!(
            run_label = %close.run_label,
            checkpoint_action = %checkpoint_action,
            total_pages = total_pages,
            "crawl session closed"
        );

        Ok(summary)
    }

    /// Snapshot the collector and the error counters into a [`CrawlResult`].
    ///
    /// Shared by the success tail of [`Self::run`] and
    /// [`Self::fail_run_with_partials`]: the summary a caller sees must be
    /// identical on both paths (#2338) — one extraction, one place.
    ///
    /// Callers must have released the task context first, so every collector
    /// sender is gone and the mpsc drain terminates.
    async fn collect_result_summary(&mut self) -> CrawlResult {
        let collected_urls = std::mem::take(&mut self.collector).collect().await;
        let total_pages = collected_urls.len();
        let errors = self.error_count.load(std::sync::atomic::Ordering::SeqCst);
        let breakdown = self.collect_error_breakdown();
        CrawlResult::new(collected_urls, total_pages, errors, breakdown)
    }

    /// Collect what the run completed and wrap `err` so the partial results
    /// travel with the failure (#2338).
    ///
    /// The caller already drained the worker set and dropped the task context
    /// before this runs, so the collector's senders are gone and `collect()`
    /// terminates. The run stays FAILED: the payload does not soften the class
    /// (matrix row 34), and the checkpoint keeps for resume because the
    /// session verdict after the loop never executes on this return.
    ///
    /// The summary event is deliberately NOT the canonical `crawl completed`
    /// message — the benchmark aggregator keys on that message and a failed
    /// run must not look like a completed one.
    pub(super) async fn fail_run_with_partials(&mut self, err: CrawlError) -> CrawlError {
        let partial = self.collect_result_summary().await;
        warn!(
            collected = partial.total_pages,
            reason = %err,
            "crawl ended early — partial results travel with the failure"
        );
        CrawlError::PartialRun {
            message: err.to_string(),
            partial,
        }
    }

    /// Bound every worker wait (#509): the longest legitimate wait is one fetch
    /// — connect plus request, each with its own timeout — or a rate-limit
    /// token (delay_ms), plus grace.
    ///
    /// The worker lifecycle is rate-limit wait + fetch + post-fetch pipeline.
    /// The first two arms are covered above; the pipeline is deliberately NOT
    /// given its own additive allowance (#2338): it is CPU-bound
    /// `spawn_blocking` work on an already-fetched body, normally far faster
    /// than the grace, and inventing a pipeline constant without a
    /// measurement would be fabrication. The documented residual risk: a
    /// pipeline that runs longer than the grace is classified as a stall and
    /// fails the run — surfaced as such by [`Self::stalled`], never silently.
    ///
    /// See [`FETCH_TIMEOUT_PHASES`] for why the fetch arm is multiplied rather
    /// than taken at face value.
    pub(super) fn worker_wait_bound(config: &CrawlerConfig) -> Duration {
        Duration::from_secs(
            config
                .timeout_secs
                .max(config.delay_ms / 1000)
                .saturating_mul(FETCH_TIMEOUT_PHASES)
                .saturating_add(SHUTDOWN_GRACE_SECS),
        )
    }

    /// Record how this crawl run ended on its own `crawl_site*` span
    /// (#1610, OBS-H7).
    ///
    /// `#[instrument]` can only see arguments, so the verdict a caller cares
    /// about — did the run finish, and was it interrupted — is declared
    /// `Empty` on the span and recorded here, where the engine knows all
    /// three. Cancellation wins over the returned `Result` on purpose: an
    /// interrupted crawl still returns `Ok` (the pages collected so far are
    /// the run's result), and reporting that as a clean `ok` is exactly the
    /// "0 pages for nobody's known reason" gap this field closes.
    pub(super) fn record_run_outcome(&self, result: &Result<CrawlResult, CrawlError>) {
        let outcome = if self.cancel_token.is_cancelled() {
            "cancelled"
        } else {
            match result {
                Ok(_) => "ok",
                Err(CrawlError::Cancelled) => "cancelled",
                Err(_) => "error",
            }
        };
        tracing::Span::current().record("outcome", outcome);
    }

    /// Build the per-category error breakdown map (issue #374).
    fn collect_error_breakdown(&self) -> std::collections::BTreeMap<CrawlErrorCategory, usize> {
        CrawlErrorCategory::ALL
            .iter()
            .filter_map(|cat| {
                let count =
                    self.error_breakdown[cat.index()].load(std::sync::atomic::Ordering::SeqCst);
                (count > 0).then_some((*cat, count))
            })
            .collect()
    }

    /// Emit the structured crawl summary (issue #356 Fase 4, error breakdown #374).
    fn log_crawl_summary(&self, total_pages: usize, errors: usize, start: std::time::Instant) {
        let duration = start.elapsed();
        let succeeded = total_pages.saturating_sub(errors);
        let summary_rate = if duration.as_secs_f64() > 0.0 {
            total_pages as f64 / duration.as_secs_f64()
        } else {
            0.0
        };
        info!(
            total_pages = total_pages,
            succeeded = succeeded,
            errors = errors,
            errors_waf = self.error_breakdown[CrawlErrorCategory::Waf.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_http = self.error_breakdown[CrawlErrorCategory::Http.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_timeout = self.error_breakdown[CrawlErrorCategory::Timeout.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_network = self.error_breakdown[CrawlErrorCategory::Network.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_rate_limit = self.error_breakdown[CrawlErrorCategory::RateLimit.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_extraction = self.error_breakdown[CrawlErrorCategory::Extraction.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_internal = self.error_breakdown[CrawlErrorCategory::Internal.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            errors_panic = self.error_breakdown[CrawlErrorCategory::Panic.index()]
                .load(std::sync::atomic::Ordering::SeqCst),
            duration_secs = duration.as_secs(),
            // #1610 (OBS-H7): `duration_secs` is whole seconds, so every run
            // under one second reported 0 and the crawl cost was only
            // recoverable from the span_close. The millisecond figure makes
            // the event queryable on its own; `duration_secs` is kept
            // untouched because existing tooling reads it.
            duration_ms = duration.as_millis() as u64,
            pages_per_sec = summary_rate,
            trace_id = %self.correlation_id.trace_id(),
            "crawl completed"
        );
    }

    /// Drain spawned tasks with a bounded wait, aborting any stragglers so a
    /// shutdown can never hang on a blocked worker (#509).
    ///
    /// Graceful path: cancellation fires upstream, workers parked on
    /// rate-limit/governor waits return [`CrawlError::Cancelled`] promptly,
    /// and the drain finishes well within `bound`. `bound` only bites when a
    /// worker is genuinely wedged (e.g. a fetch ignoring its own timeout).
    async fn drain_tasks(
        tasks: &mut tokio::task::JoinSet<Result<(), CrawlError>>,
        error_count: &Arc<AtomicUsize>,
        error_breakdown: &Arc<[AtomicUsize; 8]>,
        bound: Duration,
    ) {
        let drained = tokio::time::timeout(bound, async {
            while let Some(result) = tasks.join_next().await {
                handle_crawl_result(result, error_count, error_breakdown);
            }
        })
        .await
        .is_ok();

        if !drained {
            let remaining = tasks.len();
            warn!(
                remaining_tasks = remaining,
                bound_secs = bound.as_secs(),
                "task drain timed out — aborting remaining workers"
            );
            tasks.abort_all();
            while let Some(result) = tasks.join_next().await {
                handle_crawl_result(result, error_count, error_breakdown);
            }
        }
    }

    /// Graceful shutdown — drop the collector sender, receiver drains remaining items
    ///
    /// Performs NO checkpoint I/O: every write/delete was already executed by
    /// a decided verdict (crawl-loop periodic/signal saves, and the close
    /// decision executed by `CrawlSession::finish`).
    /// Undecided paths — a seed rejected by pattern filters, or `run()`
    /// returning `Err` — must not invent a checkpoint here (#1823).
    pub async fn shutdown(mut self) {
        // Unblock any worker still parked on a rate-limit/governor wait (#509).
        self.cancel_run();

        // Abort signal handler to prevent the runtime from hanging
        if let Some(handle) = self.signal_handle.take() {
            handle.abort();
        }

        // Same for the RAM autoscale probe (#1941): an unowned 5s poll loop
        // would outlive the crawl and keep the runtime busy.
        if let Some(handle) = self.autoscale_handle.take() {
            handle.abort();
        }

        info!("Engine shutdown complete");
    }
}
