#!/usr/bin/env bash
# Semantics harness for the docs-claims gate (scripts/check_docs_claims.sh).
#
# A docs gate that only ever runs on a healthy README is indistinguishable from
# no gate: it exits 0 on the exact tree it was written against and nobody ever
# sees it red. The three months of "`tests-1,337`" prove the point — the claim was
# wrong for 90 days because nothing evaluated it.
#
# These cases pin the behavior that cannot be proven by reading the script:
#
#   1. it PASSES on a consistent fixture (correctness, not an allowlist);
#   2. it FAILS on an edge drawn but absent from the manifest — the exact
#      defect of the old "cli → mcp → core" diagram;
#   3. it FAILS when a real production edge is missing from the diagram;
#   4. it FAILS on a static metric badge ("tests-1,337");
#   5. it FAILS when the MSRV badge contradicts rust-version;
#   6. it FAILS on a retired claim (the resurrect-a-lie case);
#   7. it fails CLOSED when README.md is missing — a guard that degrades to
#      "skip" when blind reports safety it never established.
#
# Everything runs against mktemp fixtures. No network, no mutation of the
# repository this harness runs from.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASSED=0
FAILED=0

pass() { PASSED=$((PASSED + 1)); printf '  ok   %s\n' "$1"; }
fail() { FAILED=$((FAILED + 1)); printf '  FAIL %s\n' "$1"; }

# new_fixture <name>: minimal tree the gate can read.
new_fixture() {
  local dir="$WORK/$1"
  mkdir -p "$dir/scripts" "$dir/crates/webfang_core" "$dir/crates/webfang_cli"
  cp "$REPO_ROOT/scripts/check_docs_claims.sh" "$dir/scripts/"
  chmod +x "$dir/scripts/check_docs_claims.sh"

  cat > "$dir/Cargo.toml" <<'EOF'
[workspace]
members = []
[workspace.package]
rust-version = "1.88"
EOF

  cat > "$dir/crates/webfang_core/Cargo.toml" <<'EOF'
[package]
name = "webfang_core"
[dependencies]
tokio = "1"
EOF

  cat > "$dir/crates/webfang_cli/Cargo.toml" <<'EOF'
[package]
name = "webfang_cli"
[dependencies]
webfang_core = { path = "../webfang_core" }
EOF

  cat > "$dir/README.md" <<'EOF'
# fixture

[![Rust](https://img.shields.io/badge/rust-1.88+-orange)](https://rust-lang.org)

## Architecture

```
webfang_cli ──→ webfang_core
```

## Next
EOF
  printf '%s\n' "$dir"
}

# expect <fixture> <0|1> <name>
expect() {
  local dir="$1" want="$2" name="$3" got=0
  bash "$dir/scripts/check_docs_claims.sh" >"$dir/out.log" 2>&1 || got=$?
  if [[ $got -eq $want ]]; then
    pass "$name (exit $got)"
  else
    fail "$name (esperaba exit $want, obtuve $got)"
    sed -n '1,5p' "$dir/out.log" | sed 's/^/       /'
  fi
}

printf 'check_docs_claims semantics\n'

# 1. consistent tree passes
d="$(new_fixture consistent)"
expect "$d" 0 "árbol consistente"

# 2. drawn edge that does not exist in any manifest
d="$(new_fixture phantom-edge)"
cat > "$d/README.md" <<'EOF'
# fixture

## Architecture

```
webfang_cli ──→ webfang_mcp ──→ webfang_core
```

## Next
EOF
expect "$d" 1 "arista fantasma dibujada"

# 3. real production edge missing from the diagram
d="$(new_fixture missing-edge)"
cat > "$d/README.md" <<'EOF'
# fixture

## Architecture

```
webfang_core
```

## Next
EOF
expect "$d" 1 "arista de producción ausente del diagrama"

# 4. static metric badge
d="$(new_fixture static-badge)"
cat > "$d/README.md" <<'EOF'
# fixture

[![Tests](https://img.shields.io/badge/tests-1%2C337+-green)](#testing)

## Architecture

```
webfang_cli ──→ webfang_core
```

## Next
EOF
expect "$d" 1 "badge estático de tests"

# 5. MSRV badge contradicts rust-version
d="$(new_fixture msrv-drift)"
cat > "$d/README.md" <<'EOF'
# fixture

[![Rust](https://img.shields.io/badge/rust-1.79+-orange)](https://rust-lang.org)

## Architecture

```
webfang_cli ──→ webfang_core
```

## Next
EOF
expect "$d" 1 "badge de Rust contra rust-version"

# 6. retired claim resurrected
d="$(new_fixture retired-claim)"
cat > "$d/README.md" <<'EOF'
# fixture

## Architecture

```
webfang_cli ──→ webfang_core
```

**Dependency direction:** CLI → MCP → Core.

## Next
EOF
expect "$d" 1 "afirmación retirada reintroducida"

# 7. fails closed without README
d="$(new_fixture no-readme)"
rm -f "$d/README.md"
expect "$d" 1 "sin README (fail-closed)"

printf '\n%d passed, %d failed\n' "$PASSED" "$FAILED"
(( FAILED == 0 )) || exit 1
