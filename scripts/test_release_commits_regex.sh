#!/usr/bin/env bash
# Semantics harness for the `release_commits` filter of release-plz.toml.
#
# WHY THIS EXISTS
#   `release_commits` is the only thing between "a commit landed on main" and
#   "a binary release was published". It lives in a TOML string that nothing
#   compiles, and its failure mode is silent: a regex that is one `?` away from
#   correct either releases everything (noise) or nothing (a broken release
#   pipeline that only shows up when a release is needed).
#
#   Measured: without the filter, release-plz bumped on every commit (the
#   official default — "release-plz updates the package on every commit"), and
#   v7.7.2 was proposed for a single `test:` commit (#2782).
#
# WHAT IT PINS
#   - the regex is READ from release-plz.toml, never copied: a duplicated
#     pattern keeps passing while the config is broken;
#   - the table: which subjects may cut a version and which may not;
#   - the two invariants that are easy to lose: a `test:`/`docs:`/`ci:` commit
#     never releases, and ANY type releases when it is breaking (`!`, with or
#     without scope), because the repo's convention puts the canonical marker in
#     the squash subject (AGENTS.md, "Breaking change");
#   - no `(?m)`: a `feat:` that appears only in the body must not match, which
#     is what protects against multi-commit squash bodies.
#
# ENGINE: python3 + tomllib to read the config, `re` to match. The pattern uses
# no lookarounds, no backreferences and no class escapes beyond `\(`/`\)`, the
# subset where `re` and the Rust `regex` crate release-plz uses agree.
#
# Offline: no network, no git, no repo state touched.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CONFIG="$REPO_ROOT/release-plz.toml"

if [[ ! -f "$CONFIG" ]]; then
  printf '::error::the harness has no subject: %s is missing\n' "$CONFIG" >&2
  printf '\nharness: 0 ok, 1 fail\n'
  exit 1
fi

exec python3 - "$CONFIG" <<'PY'
import re
import sys
import tomllib

path = sys.argv[1]

try:
    with open(path, "rb") as handle:
        config = tomllib.load(handle)
except (OSError, tomllib.TOMLDecodeError) as exc:
    print(f"::error::release-plz.toml could not be parsed: {exc}")
    print("\nharness: 0 ok, 1 fail")
    sys.exit(1)

workspace = config.get("workspace", {})
if "release_commits" not in workspace:
    # Fail closed: a missing key is the default behaviour (every commit
    # releases), and a `None` that silently matched nothing would look green.
    print("::error::release-plz.toml has no [workspace] release_commits: "
          "the filter is absent and every commit would cut a release")
    print("\nharness: 0 ok, 1 fail")
    sys.exit(1)

pattern = workspace["release_commits"]
print(f"pattern: {pattern}")

# (subject, must match, why)
CASES = [
    # ── the types that always release ────────────────────────────────────────
    ("feat: add --jitter-ms to the crawler", True, "feat releases"),
    ("feat(crawler): add --jitter-ms", True, "feat with scope releases"),
    ("feat!: drop the legacy extractor", True, "breaking feat (no scope)"),
    ("feat(crawler)!: drop the legacy extractor", True, "breaking feat with scope"),
    ("fix: keep the pacing when the RNG cannot be seeded", True, "fix releases"),
    ("fix(cli)!: eliminar ProviderKind::LocalOnnx", True, "breaking fix, real subject (#2391 series)"),
    ("perf: reuse the parsed DOM across passes", True, "perf releases"),
    ("revert: feat: add --jitter-ms", True, "a conventional revert releases"),
    # ── dependency bumps that change the binary ──────────────────────────────
    ("chore(deps): bump rustix a 1.2.3 por RUSTSEC-2026-1", True, "deps bump releases (security)"),
    ("chore(deps)!: bump rustix a 1.2.3", True, "breaking deps bump releases"),
    # ── ANY type when it is breaking ─────────────────────────────────────────
    ("refactor(exporter)!: skipped_non_utf8 travels in the export outcome", True, "breaking refactor, real subject (#2391 -> v7.0.0)"),
    ("refactor(cli)!: retira --enable-ai de los dos binarios MCP", True, "breaking refactor, real subject (#2145 -> v5.0.0)"),
    ("docs!: rewrite the install guide for the new flags", True, "any type releases when breaking"),
    ("chore(api)!: remove the legacy MCP field", True, "any type releases when breaking"),
    ("style!: normalize the error strings", True, "any type releases when breaking"),
    # ── multi-line messages: the subject is what counts ──────────────────────
    ("feat: add --jitter-ms\n\nthe body explains why", True, "a body does not hide the subject"),
    # ── never release ────────────────────────────────────────────────────────
    ("test: gate the resume tests that reach copy_file_range", False, "test: never releases"),
    ("docs: document the retry classification", False, "docs: never releases"),
    ("ci: install nasm for the cfg(windows) cross-lint", False, "ci: never releases"),
    ("style: rustfmt the new module", False, "style: never releases"),
    ("ci(release): filter which commits can cut a release", False, "this PR's own subject must not release"),
    ("chore: release v7.7.2", False, "release-plz's own commit never releases"),
    ("chore: release v7.7.1 (#2780)", False, "release-plz's own commit, squashed, never releases"),
    ("chore(ci): install nasm for the cfg(windows) cross-lint", False, "a chore scope that is not deps never releases"),
    ("feature: add --jitter-ms", False, "a non-conventional type never releases"),
    ("Feat: add --jitter-ms", False, "types are case-sensitive"),
    ("feat add --jitter-ms", False, "a missing colon is not a conventional commit"),
    ("fixup! feat: add --jitter-ms", False, "an autosquash subject is not a type"),
    ("refactor: move ForbiddenLiteral to its own module", False, "a plain refactor never releases"),
    ("refactor(cli): move ForbiddenLiteral to its own module", False, "a plain refactor with scope never releases"),
    ("Refactor!: move ForbiddenLiteral to its own module", False, "an uppercase type never releases"),
    ("refactor !: move ForbiddenLiteral to its own module", False, "a space before the ! is not a marker"),
    ("test: adjust the harness\n\nfeat: this one is only in the body", False, "no (?m): a feat: in the body must not match"),
]

try:
    regex = re.compile(pattern)
except re.error as exc:
    print(f"::error::release_commits is not a valid regex: {exc}")
    print("\nharness: 0 ok, 1 fail")
    sys.exit(1)

passed = 0
failed = 0
for subject, expected, why in CASES:
    got = regex.search(subject) is not None
    label = subject.replace("\n", "\\n")
    if got == expected:
        passed += 1
        print(f"  ok   {label!r} -> {'match' if got else 'no match'} ({why})")
    else:
        failed += 1
        print(f"  FAIL {label!r} -> expected {'match' if expected else 'no match'}, "
              f"got {'match' if got else 'no match'} ({why})")

print(f"\nharness: {passed} ok, {failed} fail")
sys.exit(1 if failed else 0)
PY