//! The connection-failure classifier, re-exported (#2784).
//!
//! The classifier now lives in the domain (`domain::error::cause`), next to the
//! vocabulary it produces. It moved because the **application**-layer HTTP
//! client classifies at its own send boundary too, and `application` must not
//! import `infrastructure` — the layering is inward-only. This module keeps the
//! infrastructure-side path (`infrastructure::downloader::cause`) stable for the
//! downloader and its tests.

pub(crate) use crate::domain::error::cause::FetchPhase;
