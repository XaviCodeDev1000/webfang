use super::*;
use crate::infrastructure::observability::memory_probe;

#[test]
fn probe_robots_cache_growth_5k_hosts() {
    const N: usize = 5_000;
    let cache = new_robots_cache();
    let before = memory_probe::rss_bytes();

    for i in 0..N {
        // AllowAll is the real cached fail-open state for hosts without
        // robots.txt; Rules entries are strictly larger so this is a
        // conservative lower bound per host.
        cache.insert(
            format!("probe-{i}.example.com"),
            init_cache_cell(RobotsCacheEntry::AllowAll),
        );
    }

    let after = memory_probe::rss_bytes();
    assert_eq!(cache.len(), N);
    memory_probe::append_report(
        "BEFORE — robots cache",
        &format!(
            "entries={} (AllowAll lower bound) rss_before={} rss_after={} delta={}",
            cache.len(),
            memory_probe::fmt_rss(before),
            memory_probe::fmt_rss(after),
            memory_probe::fmt_rss(after.and_then(|a| before.map(|b| a.saturating_sub(b)))),
        ),
    );
}
