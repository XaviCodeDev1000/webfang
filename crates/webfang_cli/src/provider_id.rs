//! Stable provider ids derived from an endpoint URL.

/// A stable, readable provider id derived from the endpoint URL.
///
/// FNV-1a over the URL, formatted as 64 bits: deterministic across runs and
/// toolchain versions (unlike `DefaultHasher`, which std documents as not
/// stable across Rust releases), and wide enough that two endpoints do not
/// collide in any realistic config. Not a cryptographic choice — a naming
/// convenience the operator can override with `--id`. Because the id is
/// derived from the URL, a repeat `connect` of the same URL names the same
/// record; `write_provider` then MERGES into it field by field (refreshing the
/// probe-owned keys, preserving the operator-owned ones) instead of replacing
/// it or appending a duplicate.
pub(crate) fn generated_provider_id(url: &str) -> String {
    // FNV-1a, 64-bit.
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut hash = OFFSET;
    for byte in url.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    format!("connect-{hash:016x}")
}

/// `generated_provider_id` is the fallback identity `webfang connect` writes
/// when the operator gave no `--id`: the SAME endpoint must always resolve to
/// the SAME provider id, or a reconnect would append a second record for one
/// endpoint instead of updating the existing one.
#[cfg(test)]
mod tests {
    use super::generated_provider_id;

    #[test]
    fn the_same_url_always_yields_the_same_id() {
        let url = "https://api.example.test/v1";
        assert_eq!(
            generated_provider_id(url),
            generated_provider_id(url),
            "the FNV-1a hash must be deterministic across calls"
        );
    }

    #[test]
    fn the_id_is_connect_prefixed_with_16_lowercase_hex_digits() {
        let id = generated_provider_id("https://api.example.test/v1");
        let hash = id
            .strip_prefix("connect-")
            .unwrap_or_else(|| panic!("the id must carry the connect- prefix, got: {id}"));
        assert_eq!(hash.len(), 16, "a 64-bit hash is 16 hex digits, got: {id}");
        assert!(
            hash.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')),
            "the hash must be lowercase hex, got: {id}"
        );
    }

    #[test]
    fn different_urls_yield_different_ids() {
        assert_ne!(
            generated_provider_id("https://api.example.test/v1"),
            generated_provider_id("https://other.example.test/v1"),
            "two distinct endpoints must not collide on one provider id"
        );
    }
}
