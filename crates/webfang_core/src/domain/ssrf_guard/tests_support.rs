use super::*;

pub(super) fn addr(s: &str) -> IpAddr {
    s.parse().expect("test literals always parse")
}

/// Hand-built policy for defense-in-depth tests: the guard must hold
/// even when a `NetworkPolicy` bypassed serde validation, so these
/// fixtures are constructed directly, never parsed.
pub(super) fn hand_built_policy(cidrs: &[&str]) -> NetworkPolicy {
    hand_built_policy_with_hosts(cidrs, &[])
}

/// Same, with declared hostnames: defense-in-depth fixtures must never
/// depend on serde validation having normalized them.
pub(super) fn hand_built_policy_with_hosts(cidrs: &[&str], hosts: &[&str]) -> NetworkPolicy {
    let allow_cidrs = cidrs
        .iter()
        .map(|c| {
            crate::domain::network_policy::CidrBlock::parse(c)
                .expect("test policy entries are valid CIDRs")
        })
        .collect();
    NetworkPolicy {
        mode: crate::domain::network_policy::NetworkPolicyMode::Allowlist,
        allow_cidrs,
        allow_hosts: hosts.iter().map(|h| (*h).to_string()).collect(),
    }
}
