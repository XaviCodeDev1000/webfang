use super::*;

#[test]
fn literal_host_classification() {
    assert!(is_forbidden_literal_host("127.0.0.1"));
    assert!(is_forbidden_literal_host("169.254.169.254"));
    assert!(is_forbidden_literal_host("[::1]"));
    assert!(is_forbidden_literal_host("[::ffff:127.0.0.1]"));
    assert!(!is_forbidden_literal_host("8.8.8.8"));
    assert!(!is_forbidden_literal_host("[2606:4700::1111]"));
    assert!(!is_forbidden_literal_host("example.com"));
    assert!(!is_forbidden_literal_host("127.0.0.1.nip.io"));
}

// F-06 + F-32 (#1217): the entry guard must decode every IP-literal
// encoding the URL/HTTP stacks accept — dotted, hex, octal, bare decimal,
// abbreviated — not just what `str::parse::<IpAddr>` handles.
#[test]
fn ip_literal_parser_covers_all_encodings() {
    // (host spelling, expected canonical address)
    let cases = [
        ("127.0.0.1", "127.0.0.1"),
        ("0x7f000001", "127.0.0.1"),
        ("0X7F000001", "127.0.0.1"),
        ("2130706433", "127.0.0.1"),
        ("127.1", "127.0.0.1"),
        ("127.0.1", "127.0.0.1"),
        ("0x7f.0x0.0x0.0x1", "127.0.0.1"),
        ("017700000001", "127.0.0.1"),
        ("0177.0.0.1", "127.0.0.1"),
        ("10.0.0.1", "10.0.0.1"),
        ("0xa000001", "10.0.0.1"),
        ("169.254.169.254", "169.254.169.254"),
        ("0xa9fea9fe", "169.254.169.254"),
        ("2852039166", "169.254.169.254"),
        ("192.168.0.1", "192.168.0.1"),
        ("3232235521", "192.168.0.1"),
        ("8.8.8.8", "8.8.8.8"),
        ("0x8080808", "8.8.8.8"),
        ("134744072", "8.8.8.8"),
        ("0.0.0.0", "0.0.0.0"),
        ("0x0", "0.0.0.0"),
        ("127.0.0.1.", "127.0.0.1"),
        ("::1", "::1"),
        ("[::1]", "::1"),
        ("::ffff:127.0.0.1", "::ffff:127.0.0.1"),
        ("[::ffff:127.0.0.1]", "::ffff:127.0.0.1"),
        ("2606:4700::1111", "2606:4700::1111"),
    ];
    for (host, expected) in cases {
        let want: IpAddr = expected.parse().expect("test literals always parse");
        assert_eq!(parse_ip_literal(host), Some(want), "host {host}");
    }
}

#[test]
fn ip_literal_parser_rejects_non_literals() {
    for host in [
        "example.com",
        "127.0.0.1.nip.io",
        "",
        "1.2.3.4.5",
        "0xZZZ",
        "0x",
        "08",
        ".1.2.3",
        "127.0.0.1..",
        "+1.2.3.4",
        " 127.0.0.1",
        "0x7f000001 ",
        "fe80::1%eth0",
        "[fe80::1%25eth0]",
        "1234:::",
    ] {
        assert_eq!(parse_ip_literal(host), None, "host {host:?}");
    }
}

#[test]
fn forbidden_literal_host_covers_encoded_forms() {
    for host in [
        "127.0.0.1",
        "0x7f000001",
        "0X7F000001",
        "2130706433",
        "127.1",
        "127.0.1",
        "017700000001",
        "0177.0.0.1",
        "0x7f.0x0.0x0.0x1",
        "127.0.0.1.",
        "169.254.169.254",
        "0xa9fea9fe",
        "2852039166",
        "10.0.0.1",
        "0xa000001",
        "192.168.1.1",
        "0.0.0.0",
        "0x0",
        "255.255.255.255",
        "100.64.0.1",
        "240.0.0.1",
        "::1",
        "[::1]",
        "::ffff:127.0.0.1",
        "[::ffff:127.0.0.1]",
        "::ffff:0.0.0.0",
    ] {
        assert!(is_forbidden_literal_host(host), "forbidden literal {host}");
    }
    for host in [
        "8.8.8.8",
        "0x8080808",
        "134744072",
        "1.1.1.1",
        "93.184.216.34",
        "example.com",
        "127.0.0.1.nip.io",
        "2606:4700::1111",
        "[2606:4700::1111]",
    ] {
        assert!(!is_forbidden_literal_host(host), "allowed host {host}");
    }
}

#[test]
fn reject_url_blocks_forbidden_literals_all_forms() {
    let _guard = webfang_test_utils::EnvGuard::clean(&[DISABLE_ENTRY_GUARD_ENV]);
    for raw in [
        "http://127.0.0.1/",
        "http://0x7f000001:18888/article",
        "http://2130706433:18888/article",
        "http://127.1:18888/article",
        "http://169.254.169.254/latest/meta-data/",
        "http://10.0.0.1/",
        "http://[::ffff:127.0.0.1]/",
    ] {
        let url: url::Url = raw.parse().expect("test URLs always parse");
        let err = reject_forbidden_literal_url(&url)
            .expect_err("forbidden literal must be rejected at entry");
        assert!(
            err.to_string().contains("SSRF detectado"),
            "rejection of {raw} must carry the SSRF contract text, got: {err}"
        );
    }
}

#[test]
fn reject_url_allows_hostnames_and_public_ips() {
    let _guard = webfang_test_utils::EnvGuard::clean(&[DISABLE_ENTRY_GUARD_ENV]);
    for raw in [
        "https://example.com/",
        "http://8.8.8.8/",
        "http://93.184.216.34/",
        "http://[2606:4700::1111]/",
    ] {
        let url: url::Url = raw.parse().expect("test URLs always parse");
        assert!(
            reject_forbidden_literal_url(&url).is_ok(),
            "non-literal {raw} must pass the entry guard"
        );
    }
}
