//! The marked name lookup of [`ValidatingResolver`](super::ValidatingResolver).
//! Its own module so `ssrf.rs` stays under its grandfathered size (#2784).

use super::*;

impl ValidatingResolver {
    /// Resolves `host` through the injected [`NameSource`] (production: the
    /// GAI path, `getaddrinfo`), optionally validating every address in the
    /// answer set.
    ///
    /// Taking the source by value behind an `Arc` is what lets a test drive a
    /// fixed answer set through this exact function — and therefore exercise
    /// the join to [`Self::fail_closed_scan`] that the isolated unit tests
    /// cannot reach.
    ///
    /// # Errors
    ///
    /// Returns [`ForbiddenResolutionError`] when validating and any address
    /// in the answer set is forbidden, [`ForbiddenHostNotAllowlistedError`]
    /// when the address is allowlisted but the hostname is not declared, or
    /// the underlying `io::Error` if resolution itself fails.
    pub(super) async fn gai_lookup(
        host: String,
        validate: bool,
        allow_loopback: bool,
        policy: Arc<NetworkPolicy>,
        name_source: Arc<dyn NameSource>,
    ) -> Result<Addrs, BoxedLookupError> {
        // #2784: the resolution is MARKED while it is in flight, so a failure
        // that arrives before it finishes is a RESOLUTION failure.
        let mut mark = crate::domain::error::resolution::resolution_tracker().begin(&host);
        let addrs = match name_source.lookup(host.clone()).await {
            Ok(addrs) => {
                // The name came back: record it, so a CONCURRENT request whose
                // own resolution hangs is not mistaken for this one's (#2784).
                mark.finished(true);
                addrs
            },
            Err(err) => {
                mark.finished(false);
                return Err(
                    crate::domain::error::mapping::dns_resolution_failure(err) as BoxedLookupError
                );
            },
        };

        if validate {
            Self::fail_closed_scan(&host, &addrs, allow_loopback, &policy)?;
        }

        Ok(Box::new(addrs.into_iter()) as Addrs)
    }
}
