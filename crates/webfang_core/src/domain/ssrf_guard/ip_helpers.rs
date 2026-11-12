use super::*;

/// Returns the IPv4 address embedded in a NAT64-translated address when
/// `v6` falls within the well-known prefix 64:ff9b::/96 (RFC 6052 §2.1);
/// `None` otherwise. The embedded IPv4 occupies segments\[6..8\].
///
/// A Network-Specific Prefix (the rest of 64:ff9b::/32) carries NO decodable
/// embedded address — see [`is_ipv6_nat64_non_canonical`] — so it yields
/// `None` as well.
#[must_use]
pub fn ipv6_nat64_embedded_v4(v6: &Ipv6Addr) -> Option<Ipv4Addr> {
    if !is_ipv6_nat64_well_known_prefix(v6) {
        return None;
    }
    let segments = v6.segments();
    Some(Ipv4Addr::new(
        (segments[6] >> 8) as u8,
        segments[6] as u8,
        (segments[7] >> 8) as u8,
        segments[7] as u8,
    ))
}

/// Whether `v6` falls within the canonical NAT64 Well-Known Prefix
/// 64:ff9b::/96 (RFC 6052 §2.1).
///
/// The WKP is the only NAT64 prefix whose embedded IPv4 occupies a fixed
/// position (segments\[6..8\]), so it is the only one whose embedded address
/// can be located — and therefore validated.
fn is_ipv6_nat64_well_known_prefix(v6: &Ipv6Addr) -> bool {
    let segments = v6.segments();
    segments[0] == 0x0064 && segments[1] == 0xff9b && segments[2..6].iter().all(|&s| s == 0)
}

/// Whether `v6` falls within the NAT64 prefix space 64:ff9b::/32 (RFC 6052
/// §2.1) — the Well-Known Prefix and the Network-Specific Prefixes alike.
fn is_ipv6_nat64_prefix_space(v6: &Ipv6Addr) -> bool {
    let segments = v6.segments();
    segments[0] == 0x0064 && segments[1] == 0xff9b
}

/// Returns `true` if `v6` falls within the NAT64 prefix space 64:ff9b::/32
/// (RFC 6052 §2.1) but OUTSIDE the canonical Well-Known Prefix 64:ff9b::/96
/// — i.e. a Network-Specific Prefix (RFC 6052 §2.2).
///
/// Such an address cannot be validated: RFC 6052 §2.2 places the embedded
/// IPv4 immediately after the prefix, whose length varies (/32../64), and the
/// address does NOT encode that length — so the position of the embedded IPv4
/// is not derivable from the address alone. The only safe verdict is to
/// refuse the whole space, the same fail-closed reason as Teredo (whose
/// embedded address is XOR-obfuscated and cannot be trusted either).
///
/// The canonical Well-Known Prefix is EXCLUDED on purpose: it is the
/// standard, legitimate path — `64:ff9b::8.8.8.8` is the NAT64 form of a
/// public address, and IPv6-only deployments scrape public sites through a
/// NAT64 gateway — and its verdict derives from the embedded IPv4, which IS
/// validated ([`ipv6_nat64_embedded_v4`]).
#[must_use]
pub fn is_ipv6_nat64_non_canonical(v6: &Ipv6Addr) -> bool {
    is_ipv6_nat64_prefix_space(v6) && !is_ipv6_nat64_well_known_prefix(v6)
}

/// Returns the IPv4 address encapsulated in a 6to4 address (RFC 3056) when
/// `v6` falls within 2002::/16; `None` otherwise. The embedded IPv4 bytes
/// occupy the high byte of segments\[1\], the low byte of segments\[1\], the
/// high byte of segments\[2\], and the low byte of segments\[2\] — e.g.
/// `2002:7f00:1::` embeds 127.0.0.1.
#[must_use]
pub fn ipv6_6to4_embedded_v4(v6: &Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = v6.segments();
    if segments[0] != 0x2002 {
        return None;
    }
    Some(Ipv4Addr::new(
        (segments[1] >> 8) as u8,
        segments[1] as u8,
        (segments[2] >> 8) as u8,
        segments[2] as u8,
    ))
}

/// Returns the IPv4 address encapsulated in an ISATAP address (RFC 5214
/// §6.1) when `v6` carries the ATOP identifier; `None` otherwise.
///
/// RFC 5214 §6.1 builds the ISATAP interface identifier in Modified EUI-64
/// format by concatenating the 24-bit IANA OUI, the 16-bit ATOP identifier
/// `0101111011111110` (0x5efe) and the 32-bit IPv4 address:
///
/// ```text
///   |000000ug00000000|0101111011111110|mmmmmmmmmmmmmmmmmmmmmmmmmmmmmmmm|
///   \____ 24 bits ___/\___ ATOP _______/\______ IPv4 address _________/
/// ```
///
/// The interface identifier is the last 64 bits of the address, so the ATOP
/// identifier sits at bits 80..96 (segments\[5\]) and the encapsulated IPv4
/// occupies the LOW 32 bits (segments\[6..8\]) — the very same fixed
/// position the NAT64 Well-Known Prefix uses. Unlike a NAT64
/// Network-Specific Prefix, the ISATAP position never moves, which is
/// exactly why the encapsulated address can be located and validated
/// (#2232).
///
/// Only the ATOP identifier is checked: the prefix in front of it is the
/// site's own prefix, and the `u`/`g` bits of the OUI field vary per
/// interface. A false positive (a public address that happens to carry
/// `0x5efe` there) is re-validated as the IPv4 it encapsulates, like every
/// other translation form.
#[must_use]
pub fn ipv6_isatap_embedded_v4(v6: &Ipv6Addr) -> Option<Ipv4Addr> {
    let segments = v6.segments();
    if segments[5] != 0x5efe {
        return None;
    }
    Some(Ipv4Addr::new(
        (segments[6] >> 8) as u8,
        segments[6] as u8,
        (segments[7] >> 8) as u8,
        segments[7] as u8,
    ))
}

/// Returns `true` if `v6` falls within the Teredo range 2001:0000::/32
/// (RFC 4380).
#[must_use]
pub fn is_ipv6_teredo(v6: &Ipv6Addr) -> bool {
    let segments = v6.segments();
    segments[0] == 0x2001 && segments[1] == 0x0000
}

/// Returns `true` if `host` is a literal IP address within a forbidden range.
///
/// Accepts bare IPv4 literals in ALL encodings (dotted decimal, hex `0x…`,
/// bare decimal, octal `0…`, abbreviated `a.b` / `a.b.c` forms — see
/// [`parse_ip_literal`]), bare IPv6 literals, and the bracketed form
/// produced by `http::Uri::host()` (e.g. `[::1]`). Hostnames return `false` —
/// synchronous redirect callbacks cannot resolve DNS; hostnames are validated
/// at entry by the async SSRF guard. Zone-id IPv6 literals (`[fe80::1%25eth0]`)
/// fail to parse as plain IPs and are treated as non-literals.
#[must_use]
pub fn is_forbidden_literal_host(host: &str) -> bool {
    parse_ip_literal(host).is_some_and(|ip| is_forbidden_ip(&ip))
}

/// Parameterized literal-host verdict (#1462): [`is_forbidden_literal_host`]
/// with the per-client loopback permit threaded through.
///
/// Alternate spellings of a permitted address (`0x7f000001`, `127.1`) stay
/// permitted — the permit is on the address, not its spelling — while every
/// other forbidden literal stays denied even with the flag set.
#[must_use]
pub fn is_forbidden_literal_host_with(host: &str, allow_loopback: bool) -> bool {
    parse_ip_literal(host).is_some_and(|ip| is_forbidden_ip_with(&ip, allow_loopback))
}

/// Policy-parameterized literal-host verdict (B2 slice 2): the entry gate
/// for a provider that declares a [`NetworkPolicy`].
///
/// Required, not merely nice: wreq's connector parses IP-literal hosts
/// directly and never invokes the custom resolver, so a literal target is
/// invisible to every later layer — without this the whole literal half of
/// the allowlist would be unreachable *by construction*, whatever the
/// resolver does.
///
/// The `allow_hosts` half deliberately does NOT apply here: a literal is not
/// a declared hostname (validation rejects an IP literal in `allow_hosts`),
/// and the LAN use case addresses the box directly, so `allow_cidrs` alone
/// opens it. That asymmetry is pinned by
/// `literal_spelling_needs_no_allow_hosts_on_the_resolved_path`.
#[must_use]
pub fn is_forbidden_literal_host_with_policy(
    host: &str,
    allow_loopback: bool,
    policy: &NetworkPolicy,
) -> bool {
    parse_ip_literal(host)
        .is_some_and(|ip| is_forbidden_ip_with_policy(&ip, allow_loopback, policy))
}

/// Resolved-address verdict for a dial by name (B2 slice 2) — the AND
/// decision.
///
/// [`is_forbidden_ip_with_policy`] answers "is this address allowed at
/// all?"; this one additionally answers "is THIS name allowed to reach it?".
/// A hostname dial into an allowlisted network requires **both** the
/// resolved IP in `allow_cidrs` and the hostname in `allow_hosts`:
///
/// - **AND** is the only reading that gives `allow_hosts` a security
///   function. Under OR, allowing `10.0.0.0/8` opens the whole range to any
///   domain that resolves there — the classic DNS-rebinding shape, where an
///   attacker-controlled name points at your LAN and walks straight through
///   the allowlist. The allowlisted network is the *precondition*; the
///   declared hostname is the *permission*.
///
/// A host that is an IP *literal* skips the `allow_hosts` requirement: the
/// address is the whole identity of the target, so there is no name for an
/// attacker to rebind.
///
/// Evaluation order is the same security-first ladder as
/// [`is_forbidden_ip_with_policy`]: always-denied set, then the exclusive
/// loopback flag, then public addresses, then the NAT64/6to4/ISATAP embedded
/// IPv4, and only then the allowlist plus the host gate.
#[must_use]
pub fn is_forbidden_resolved_address(
    ip: &IpAddr,
    host: &str,
    allow_loopback: bool,
    policy: &NetworkPolicy,
) -> bool {
    if is_always_denied_ip(ip) {
        return true;
    }
    if allow_loopback && is_permitted_loopback(ip) {
        return false;
    }
    if !is_forbidden_ip(ip) {
        return false;
    }
    if let IpAddr::V6(v6) = ip {
        if let Some(v4) = ipv6_nat64_embedded_v4(v6)
            .or_else(|| ipv6_6to4_embedded_v4(v6))
            .or_else(|| ipv6_isatap_embedded_v4(v6))
        {
            // Same invariant as in `is_forbidden_ip_with_policy`: the
            // loopback exemption does not travel through a translation
            // prefix (#1965, A2). This is the live path — the resolver and
            // the redirect policy both dial through it.
            if is_permitted_loopback(&IpAddr::V4(v4)) {
                return true;
            }
            return is_forbidden_resolved_address(&IpAddr::V4(v4), host, allow_loopback, policy);
        }
    }
    // Only an allowlist entry can reach this point — and it is the
    // precondition, not the permission.
    if !policy.allows_ip(ip) {
        return true;
    }
    let host_is_literal = parse_ip_literal(host).is_some();
    !(host_is_literal || policy.allows_host(host))
}

/// Parses `host` as an IP literal in every encoding the URL/HTTP stacks
/// accept, returning `None` for hostnames.
///
/// Beyond the standard [`IpAddr`] syntax (dotted-decimal IPv4, all IPv6
/// forms), this covers the legacy `inet_aton` / WHATWG-URL IPv4 encodings
/// that the `url` crate normalizes (and that `getaddrinfo` resolves when a
/// raw `http::Uri` host reaches the socket layer):
///
/// - hexadecimal parts (`0x7f000001`, `0x7f.0x0.0x0.0x1`),
/// - octal parts (`017700000001`, `0177.0.0.1`),
/// - a bare 32-bit decimal (`2130706433`),
/// - abbreviated 2- and 3-part forms (`127.1`, `127.0.1`).
///
/// A single trailing dot (FQDN root, `127.0.0.1.`) is stripped before
/// parsing. Strings containing `:` that fail strict IPv6 parsing (e.g. zone
/// ids) return `None` — they are never fed to the IPv4 parser.
#[must_use]
pub fn parse_ip_literal(host: &str) -> Option<IpAddr> {
    let literal = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = literal.parse::<IpAddr>() {
        return Some(ip);
    }
    if literal.contains(':') {
        return None;
    }
    parse_ipv4_all_forms(literal).map(IpAddr::V4)
}

/// Parses the legacy `inet_aton` / WHATWG-URL IPv4 encodings: 1–4 dot-separated
/// parts, each hex (`0x…`), octal (leading `0`), or decimal, with the last part
/// filling every remaining byte (`127.1` → `127.0.0.1`, `2130706433` →
/// `127.0.0.1`). Returns `None` unless the whole string is a valid encoding —
/// partial garbage (`0xZZZ`, `08`, `1.2.3.4.5`, empty parts) never parses, so a
/// forged host can at worst fall through to the hostname path, where the
/// connect-time resolver fails it closed.
fn parse_ipv4_all_forms(text: &str) -> Option<Ipv4Addr> {
    let text = text.strip_suffix('.').unwrap_or(text);
    if text.is_empty() {
        return None;
    }
    let parts: Vec<&str> = text.split('.').collect();
    if parts.len() > 4 || parts.iter().any(|part| part.is_empty()) {
        return None;
    }
    let mut numbers = Vec::with_capacity(parts.len());
    for part in &parts {
        numbers.push(parse_ipv4_number(part)?);
    }
    let last_index = numbers.len() - 1;
    for (index, number) in numbers.iter().enumerate() {
        // Every leading part fills exactly one octet; the last part fills the
        // remaining bytes (`256^(5 - len)`), mirroring the WHATWG IPv4 parser.
        let ceiling = if index == last_index {
            256_u64.pow((5 - numbers.len()) as u32)
        } else {
            256
        };
        if *number >= ceiling {
            return None;
        }
    }
    let mut value: u64 = numbers[last_index];
    for (index, number) in numbers[..last_index].iter().enumerate() {
        value += number << (8 * (3 - index));
    }
    Some(Ipv4Addr::from(value as u32))
}

/// Parses one dot-separated IPv4 part with radix detection: `0x…`/`0X…` hex,
/// leading-`0` octal, otherwise decimal. Strict — the whole part must be valid
/// digits for its radix (`+1`, whitespace, empty hex/octal bodies reject).
fn parse_ipv4_number(part: &str) -> Option<u64> {
    let (radix, digits) =
        if let Some(hex) = part.strip_prefix("0x").or_else(|| part.strip_prefix("0X")) {
            (16_u32, hex)
        } else if part.len() > 1 && part.starts_with('0') {
            (8_u32, part)
        } else {
            (10_u32, part)
        };
    if digits.is_empty() {
        return None;
    }
    let all_valid = match radix {
        16 => digits.chars().all(|c| c.is_ascii_hexdigit()),
        8 => digits.chars().all(|c| matches!(c, '0'..='7')),
        _ => digits.chars().all(|c| c.is_ascii_digit()),
    };
    if !all_valid {
        return None;
    }
    u64::from_str_radix(digits, radix).ok()
}

#[cfg(test)]
#[path = "tests_parity.rs"]
mod tests_parity;
