#!/usr/bin/env python3
"""Reproducible re-audit of PRD-MIG-2026-01 with fail-closed verification.

Why this exists
---------------
The PRD v1 measured ~60 facts against one SHA and copied them into prose. This
repo publishes several versions a day: between the v1 baseline and today the
workspace went from 7.7.1 to 8.0.0, 8.0.1 and 8.0.2. The v1 rule itself
("if the repo moves, re-run the script") fired and nobody ran it. Re-running
the script by hand does NOT fix that: the next audit is also a snapshot, and
it ages too.

The structural fix is taking the facts away from the document:

  * `--write`  re-derives ALL the facts from the tree and freezes them into
               evidence/mig-2026-01/baseline.json (the F0 evidence).
  * `--check`  verifies that the PRD v2 contains no literal facts, that every
               {{BASELINE.path}} resolves, that it cites no nonexistent RFs,
               that the SHA is still HEAD, an ancestor, or the tree of a
               squash, and that the DECISIONS the v1 wrote by hand (major,
               backport line, Miri gate, edition flip) are still the ones
               DERIVED from the repo's living policies.

`--check` never compares a counter against a frozen number: counters are F0
evidence, not a gate over main. It does compare a small set of facts that
*invalidate decisions* (version, channel, support lines, Miri policy,
edition). With `TRAIN_OPEN` present that drift is blocking; without the
marker it degrades to a NOTE so the repo does not go red once the train is
closed.

Usage
-----
  python3 scripts/audit_migration_baseline.py --write evidence/mig-2026-01/baseline.json
  python3 scripts/audit_migration_baseline.py --check evidence/mig-2026-01/baseline.json
  python3 scripts/audit_migration_baseline.py --check ... --json   # summary

Needs no dependencies beyond the stdlib, writes to the repo only under
`--write`, and needs no cargo/rustc (important: this sandbox has neither).
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

SCHEMA = 1
DEFAULT_EVIDENCE = "evidence/mig-2026-01/baseline.json"
DEFAULT_PRD = "evidence/mig-2026-01/PRD-v2.md"
DEFAULT_MARKER = "evidence/mig-2026-01/TRAIN_OPEN"

# Facts that, when they change, invalidate a DECISION of the PRD (not a counter).
DECISION_FACTS = [
    "workspace_version",
    "toolchain.channel",
    "toolchain.rust_version",
    "edition.workspace",
    "support.stable",
    "support.maintenance",
    "support.eol_first",
    "miri.policy",
    "distribution.crates_io",
]

# Numbers/phrases from v1 that must never be written into the v2 again. The v2
# expresses them as {{BASELINE.*}} or as a derived decision. A list of regexes.
BANNED_LITERALS = [
    r"v7\.7\.1",
    r"\b8\.0\.0\b",
    r"\b474\b",
    r"\b35\b pasos",
    r"steps=35",
    r"7\.7 STABLE",
    r"7\.6 MAINTENANCE",
    r"7\.7/7\.6",
    r"1,337",
    r"Miri verified",
    r"Miri verde",
    r"workspace `7\.7\.1`",
]


# --------------------------------------------------------------------------
# reading helpers (defensive: a missing file is a fact, not a crash)
# --------------------------------------------------------------------------
def read(root: Path, rel: str) -> str:
    try:
        return (root / rel).read_text(encoding="utf-8", errors="replace")
    except OSError:
        return ""


def toml_scalar(text: str, section: str, key: str) -> str | None:
    block = re.search(
        r"(?ms)^\[" + re.escape(section) + r"\][^\n]*\n(.*?)(?=^\[|\Z)", text
    )
    if not block:
        return None
    m = re.search(
        r'(?m)^\s*' + re.escape(key) + r'\s*=\s*"([^"]*)"', block.group(1)
    )
    return m.group(1) if m else None


def git_rc(root: Path, *args: str) -> tuple[int, str]:
    """Runs git and keeps the exit code (callers fail closed)."""
    if not (root / ".git").exists():
        return 1, ""
    try:
        p = subprocess.run(
            ["git", "-C", str(root), *args],
            capture_output=True, text=True, timeout=10, check=False,
        )
        return p.returncode, p.stdout
    except (OSError, subprocess.SubprocessError):
        return 1, ""


def git(root: Path, *args: str) -> str:
    _, out = git_rc(root, *args)
    return out.strip()


def head_sha_matches_history(root: Path, sha: str, live_head: str) -> bool:
    """Accepts HEAD, real ancestry, or a SQUASH that preserved the measured tree.

    The merge queue uses SQUASH: the PR tip is not an ancestor of main, but its
    tree does survive the squash. `--write` freezes the tip ALREADY measured and
    only afterwards writes baseline.json, so between the SHA's tree and the
    squash's the only legitimate change is the baseline itself. Any other
    difference (or a git without that object) stays fail-closed.
    """
    if sha == live_head:
        return True
    rc, _ = git_rc(root, "merge-base", "--is-ancestor", sha, live_head)
    if rc == 0:
        return True
    rc, changed = git_rc(root, "diff", "--name-only", "-z", sha, live_head)
    if rc != 0:
        return False
    return all(path == DEFAULT_EVIDENCE for path in changed.split("\0") if path)


def resolve_head_sha(root: Path, explicit: str | None) -> str:
    if explicit:
        return explicit
    return git(root, "rev-parse", "HEAD") or ""


def repository_state(root: Path) -> str:
    """Classifies `root` for the freeze: repository top, nested inside one, or none.

    A `.git` entry at the root (a worktree's is a file and counts) says "top".
    A missing one is ambiguous: the no-repo fixture the harness pins is
    legitimate — ancestry is unprovable there and the --check guard already
    reports it as "measured over another tree" — but the same shape is
    reachable in production by pointing --root/MIGRATION_ROOT at a
    subdirectory, where the hazard check would be silently skipped.
    rev-parse --show-toplevel tells the two apart: success means a repository
    surrounds this root and the freeze must fail closed.
    """
    if (root / ".git").exists():
        return "top"
    try:
        p = subprocess.run(
            ["git", "-C", str(root), "rev-parse", "--show-toplevel"],
            capture_output=True, text=True, timeout=10, check=False,
        )
        if p.returncode == 0:
            return "nested"
    except (OSError, subprocess.SubprocessError):
        pass
    return "none"


# Refs that survive a squash merge: the default branch, local and remote, in
# lookup order. The merge queue uses SQUASH (.github/merge-queue.json), so the
# tip of a Release PR is NOT in the history of the default branch and never
# will be: freezing it passes the ancestry check exactly once, on the squash
# commit, and then fails on the first PR that touches any other file. Only an
# anchor reachable from the default branch survives the merge.
def durable_refs(root: Path) -> tuple[str, ...]:
    """The default branch, local and remote, detected through origin's HEAD.

    When origin (or its HEAD symref) is absent, the main/origin-main pair is
    the fallback. A ref that does not exist in this repository is skipped —
    not counted as a rejection — by existing_durable_refs.
    """
    rc, out = git_rc(root, "symbolic-ref", "--quiet", "--short", "refs/remotes/origin/HEAD")
    if rc == 0:
        remote = out.strip()
        local = remote.split("/", 1)[1] if remote.startswith("origin/") else remote
        return (local, remote) if local != remote else (remote,)
    return ("main", "origin/main")


def existing_durable_refs(root: Path) -> tuple[str, ...]:
    """The durable refs that exist in this repository, in lookup order.

    merge-base --is-ancestor exits 1 for "not an ancestor" and 128 for "no
    such ref"; only the first is evidence about the commit. A ref that
    rev-parse --verify rejects simply does not exist here.
    """
    present = []
    for ref in durable_refs(root):
        rc, _ = git_rc(root, "rev-parse", "--verify", "--quiet", ref)
        if rc == 0:
            present.append(ref)
    return tuple(present)


def durable_anchor(root: Path, sha: str) -> str | None:
    """Returns the first durable ref that `sha` is an ancestor of, or None."""
    for ref in existing_durable_refs(root):
        rc, _ = git_rc(root, "merge-base", "--is-ancestor", sha, ref)
        if rc == 0:
            return ref
    return None


def freeze_hazard(root: Path, sha: str) -> str | None:
    """Diagnosis of a frozen SHA the next squash would orphan.

    Returns None when the anchor is durable. A SHA without a durable ref is
    acceptable ONLY on a branch whose tip is not on the default branch yet
    (a Release PR before the squash): there the operator re-anchors by hand
    with --allow-unmerged-anchor and the gate re-verifies it on every later PR.
    """
    if durable_anchor(root, sha):
        return None
    rc, _ = git_rc(root, "rev-parse", "--verify", "--quiet", sha)
    if rc != 0:
        return f"head_sha {sha[:12]} is not an object of this repository"
    present = existing_durable_refs(root)
    if not present:
        return (
            f"no durable ref exists in this repository (looked for: "
            f"{', '.join(durable_refs(root))}): ancestry of head_sha "
            f"{sha[:12]} cannot be proven"
        )
    return (
        f"head_sha {sha[:12]} is not an ancestor of {' nor '.join(present)}: "
        "the next squash will orphan it and the baseline will fail on the first PR "
        "that touches any other file"
    )


def get_path(obj: object, dotted: str) -> object:
    cur = obj
    for part in dotted.split("."):
        if not isinstance(cur, dict) or part not in cur:
            raise KeyError(dotted)
        cur = cur[part]
    return cur


def render(value: object) -> str:
    if isinstance(value, list):
        return ", ".join(str(v) for v in value)
    if isinstance(value, bool):
        return "true" if value else "false"
    if value is None:
        return "null"
    return str(value)


# --------------------------------------------------------------------------
# fact derivation
# --------------------------------------------------------------------------
def support_facts(root: Path) -> dict:
    raw = read(root, ".github/support.json")
    try:
        lines = json.loads(raw or "{}").get("lines", [])
    except (ValueError, AttributeError):
        lines = []
    stable = next((x for x in lines if x.get("state") == "STABLE"), {})
    maint = next((x for x in lines if x.get("state") == "MAINTENANCE"), {})
    eol = next((x for x in lines if x.get("state") == "EOL"), {})
    return {
        "stable": stable.get("minor"),
        "stable_latest": stable.get("latest"),
        "stable_initial": (str(stable.get("minor")) + ".0") if stable.get("minor") else None,
        "maintenance": maint.get("minor"),
        "maintenance_latest": maint.get("latest"),
        "eol_first": eol.get("minor"),
        "rotation": eol.get("eol_reason"),
        "states": {x.get("minor"): x.get("state") for x in lines if x.get("minor")},
    }


def miri_facts(root: Path) -> dict:
    readme = read(root, "README.md")
    ci = read(root, ".github/workflows/ci.yml")
    san = read(root, ".github/workflows/sanitizers.yml")
    jobs = re.findall(r"(?m)^  (miri[^:]*):", ci) + re.findall(
        r"(?m)^  (miri[^:]*):", san
    )
    advisory = "Miri lanes are **advisory**" in readme or "advisory" in readme.lower() and "miri lanes" in readme.lower()
    return {
        "policy": "advisory" if advisory else "gate",
        "lanes": len(jobs),
        "jobs": sorted(set(jobs)),
        "continue_on_error": "continue-on-error" in ci,
        "never_merge_gate": "never a merge gate" in readme,
        "known_limits": [
            "servo_arc / Tree Borrows",
            "BoringSSL FFI",
            "inline asm / recent syscalls",
        ],
    }


def toolchain_pin_facts(root: Path) -> dict:
    steps = 0
    references = 0
    shas: dict[str, int] = {}
    stable: list[dict] = []
    nightly: list[dict] = []
    for wf in sorted((root / ".github/workflows").glob("*.yml")):
        text = read(root, str(wf.relative_to(root)))
        for sha in re.findall(r"dtolnay/rust-toolchain@([0-9a-f]{40})", text):
            steps += 1
            shas[sha[:12]] = shas.get(sha[:12], 0) + 1
        # Counts EVERY occurrence, including partial citations in comments:
        # v1 pinned "35" in an acceptance and the tree already measured 37.
        references += len(re.findall(r"dtolnay/rust-toolchain@", text))
        # [ \t] and not \s: \s crosses line breaks and confuses the job id
        # `toolchain:` (ci.yml) with a pin, capturing `name:` from the next
        # line. A job id is not a pin.
        for m in re.finditer(r"(?m)^([ \t]+)toolchain:[ \t]*([^\s#]+)", text):
            line = text[: m.start()].count("\n") + 1
            rec = {"file": str(wf.relative_to(root)), "line": line, "value": m.group(2)}
            (nightly if m.group(2).startswith("nightly") else stable).append(rec)
    return {
        "dtolnay_steps": steps,
        "dtolnay_references": references,
        "sha_prefixes": shas,
        "stable_explicit": stable,
        "stable_explicit_count": len(stable),
        "nightly_explicit": nightly,
        "nightly_explicit_count": len(nightly),
    }


def release_target_facts(root: Path) -> list[str]:
    text = read(root, ".github/workflows/release.yml")
    return sorted(set(re.findall(r"(?m)^\s*- target:\s*([a-z0-9_]+-[a-z0-9_]+-\S+)", text)))


def edition_facts(root: Path) -> dict:
    manifest = read(root, "Cargo.toml")
    workspace = toml_scalar(manifest, "workspace.package", "edition")
    # Only the `members` array: reading the whole [workspace] block mixes
    # resolver = "2" and the `exclude` entries (fuzz, tools/...) with the
    # members, and would make it believe there are per-crate edition overrides
    # where there are none.
    block = re.search(r"(?ms)^\s*members\s*=\s*\[(.*?)\]", manifest)
    members = re.findall(r'"([^"]+)"', block.group(1)) if block else []
    inherit, overrides = [], []
    for member in members:
        txt = read(root, f"{member}/Cargo.toml")
        if re.search(r"(?m)^\s*edition\.workspace\s*=\s*true", txt):
            inherit.append(member)
        elif re.search(r"(?m)^\s*edition\s*=", txt):
            overrides.append(member)
    excluded = {}
    for rel in ("fuzz/Cargo.toml", "tools/intra-crate-lint/Cargo.toml"):
        value = toml_scalar(read(root, rel), "package", "edition")
        if value is None:
            # those manifests declare edition before [dependencies]
            m = re.search(r'(?m)^\s*edition\s*=\s*"([^"]*)"', read(root, rel))
            value = m.group(1) if m else None
        excluded[rel] = value
    return {
        "workspace": workspace,
        "members": members,
        "members_inherit": inherit,
        "members_inherit_count": len(inherit),
        "member_overrides": overrides,
        "excluded": excluded,
    }


COUNTER_PATTERNS = {
    "assert_matches": r"assert!\s*\(\s*matches!",
    "lossy_into_owned": r"from_utf8_lossy\([^;]{0,300}?\)\s*\.\s*into_owned\(\)",
    "lossy_to_string": r"from_utf8_lossy\([^;]{0,300}?\)\s*\.\s*to_string\(\)",
    "all_into_owned": r"\.into_owned\(\)",
    "strip_prefix_unwrap_or": r"strip_prefix\([^;]{0,200}?\)\s*\.\s*unwrap_or\(",
    "if_let": r"\bif\s+let\b",
    "allow_unsafe_code": r"#!?\[allow\(unsafe_code\)\]",
    "unsafe_blocks": r"(?<!\w)unsafe\s*\{",
    "static_mut": r"\bstatic\s+mut\b",
    "rpit": r"->\s*impl\s+[A-Za-z_]",
    "gen_ident": r"\bgen\b",
    "cfg_if": r"\bcfg_if!|\bcfg_if::",
    "env_mutations": r"\b(?:env::)?(?:set_var|remove_var)\(",
}


def count_facts(root: Path) -> dict:
    files = sorted((root / "crates").rglob("*.rs"))
    texts = [p.read_text(encoding="utf-8", errors="replace") for p in files]
    out = {"rust_files": len(files)}
    for name, pattern in COUNTER_PATTERNS.items():
        rx = re.compile(pattern, re.S)
        out[name] = sum(len(rx.findall(t)) for t in texts)
    out["snapshots"] = len(list((root / "crates").rglob("*.snap")))
    out["trybuild_stderr"] = len(list((root / "crates").rglob("*.stderr")))
    out["workflows"] = len(list((root / ".github/workflows").glob("*.yml")))
    return out


SYMBOL_NEEDLES = {
    "SessionCap::try_admit": "pub(super) fn try_admit",
    "SessionCap prune loop": ".front()\n            .is_some_and",
    "InFlightGuard": "struct InFlightGuard",
    "HandlerGuard": "struct HandlerGuard",
    "HandlerGuard::join": "async fn join(mut self)",
    "env_set": "pub fn env_set(",
    "env_remove": "pub fn env_remove(",
    "EnvGuard": "pub struct EnvGuard",
    "env_invariant_scan": "Source-scan enforcement of the workspace ENV_LOCK invariant",
    "McpHandler::call_tool": "async fn call_tool(",
    "CatchPanicLayer": "use tower_http::catch_panic::CatchPanicLayer",
}


def symbol_facts(root: Path) -> dict:
    files = sorted((root / "crates").rglob("*.rs")) + sorted(
        (root / "crates").rglob("*.toml")
    )
    cache = [(p, p.read_text(encoding="utf-8", errors="replace")) for p in files]
    out = {}
    for name, needle in SYMBOL_NEEDLES.items():
        hit = next((str(p.relative_to(root)) for p, t in cache if needle in t), None)
        out[name] = hit
    return out


def quick_facts(root: Path) -> dict:
    """The 4 files the decisions depend on. Cheap enough for CI."""
    manifest = read(root, "Cargo.toml")
    toolfile = read(root, "rust-toolchain.toml")
    sup = support_facts(root)
    miri = miri_facts(root)
    return {
        "workspace_version": toml_scalar(manifest, "workspace.package", "version"),
        "toolchain.channel": toml_scalar(toolfile, "toolchain", "channel"),
        "toolchain.rust_version": toml_scalar(manifest, "workspace.package", "rust-version"),
        "edition.workspace": toml_scalar(manifest, "workspace.package", "edition"),
        "support.stable": sup["stable"],
        "support.maintenance": sup["maintenance"],
        "support.eol_first": sup["eol_first"],
        "miri.policy": miri["policy"],
        "distribution.crates_io": "Nothing is published to crates.io" not in read(root, "README.md"),
    }


def derive_decisions(quick: dict, edition: dict, sup: dict, miri: dict) -> dict:
    """RF-23/RF-24/RF-13/K9 stop being hand-written numbers."""
    try:
        next_major = int(str(quick["support.stable"]).split(".")[0]) + 1
        major_target = f"{next_major}.0.0"
    except (ValueError, AttributeError, TypeError):
        major_target = "UNRESOLVED"
    atomic = not edition["member_overrides"] and edition["members_inherit_count"] == len(
        edition["members"]
    )
    return {
        "major_target": major_target,
        "backport_line": quick["support.stable"],
        "eol_on_release": quick["support.maintenance"],
        "msrv_compat_toolchain": quick["toolchain.channel"],
        # The wording values are frozen data: baseline.json stores them and
        # --check re-derives and compares them, so translating them is a
        # decision-fact change that needs the release-process re-freeze first.
        "miri_gate_wording": (
            "advisory revisado, sin rojos nuevos atribuibles al bump"
            if miri["policy"] == "advisory"
            else "Miri verde (gate)"
        ),
        "edition_flip": "atomic" if atomic else "per-crate",
        "support_rotation": sup["rotation"],
    }


def collect(root: Path, head_sha: str) -> dict:
    manifest = read(root, "Cargo.toml")
    toolfile = read(root, "rust-toolchain.toml")
    sup = support_facts(root)
    miri = miri_facts(root)
    edition = edition_facts(root)
    pins = toolchain_pin_facts(root)
    readme = read(root, "README.md")
    quick = quick_facts(root)
    return {
        "schema": SCHEMA,
        "generated_at": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "head_sha": head_sha,
        "head_date": git(root, "show", "-s", "--format=%cI", "HEAD"),
        "head_subject": git(root, "show", "-s", "--format=%s", "HEAD"),
        "workspace_version": quick["workspace_version"],
        "toolchain": {
            "channel": quick["toolchain.channel"],
            "rust_version": quick["toolchain.rust_version"],
            "mise_rust": toml_scalar(read(root, "mise.toml"), "tools", "rust"),
            "components": re.findall(r'"([^"]+)"', (re.search(r"components\s*=\s*\[([^\]]*)\]", toolfile) or re.match(r"", "")).group(1) if re.search(r"components\s*=\s*\[([^\]]*)\]", toolfile) else ""),
        },
        "edition": edition,
        "lints": {
            "unsafe_code": toml_scalar(manifest, "workspace.lints.rust", "unsafe_code"),
            "env_mutation_owner": "webfang_test_utils",
        },
        "support": sup,
        "miri": miri,
        "distribution": {
            "kind": "binaries-only" if "binaries-only" in readme else "unknown",
            "crates_io": quick["distribution.crates_io"],
        },
        "release_targets": release_target_facts(root),
        "toolchain_pins": pins,
        "counts": count_facts(root),
        "symbols": symbol_facts(root),
        "decisions": derive_decisions(quick, edition, sup, miri),
        "decision_facts": DECISION_FACTS,
        "banned_literals": BANNED_LITERALS,
    }


# --------------------------------------------------------------------------
# fail-closed verification
# --------------------------------------------------------------------------
def verify(root: Path, evidence_path: Path, prd_path: Path,
           train_marker: Path, live_head: str) -> tuple[int, list[str], list[str], dict]:
    errors: list[str] = []
    notes: list[str] = []

    # 1. evidence readable and complete ------------------------------------
    try:
        ev = json.loads(evidence_path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        return 1, [f"baseline unreadable or invalid JSON: {exc}"], [], {}
    if ev.get("schema") != SCHEMA:
        errors.append(f"schema={ev.get('schema')!r}, expected {SCHEMA}")
    for key in ("head_sha", "decisions", "decision_facts", "banned_literals",
                "workspace_version", "toolchain", "support", "miri", "edition"):
        if key not in ev:
            errors.append(f"missing required key '{key}' in the baseline")

    # 2. the PRD cannot contain literal facts ------------------------------
    prd = ""
    if not prd_path.is_file():
        errors.append(f"PRD v2 not found at {prd_path}")
    else:
        prd = prd_path.read_text(encoding="utf-8", errors="replace")
        for literal in ev.get("banned_literals", []):
            try:
                if re.search(literal, prd):
                    errors.append(f"PRD v2 reintroduces the literal fact /{literal}/")
            except re.error as exc:
                errors.append(f"invalid regex in banned_literals: {literal}: {exc}")

        # 3. every placeholder resolves against the evidence ----------------
        placeholders = sorted(set(re.findall(r"\{\{BASELINE\.([A-Za-z0-9_.]+)\}\}", prd)))
        for dotted in placeholders:
            try:
                get_path(ev, dotted)
            except KeyError:
                errors.append(f"placeholder {{{{BASELINE.{dotted}}}}} does not resolve in baseline.json")
        notes.append(f"{len(placeholders)} placeholders resolved")

        # 4. §3 -> §6 numbering drift: no orphan RF -------------------------
        defined = set(re.findall(r"\*\*RF-(\d{2})", prd))
        mentioned = set(re.findall(r"\bRF-(\d{2})\b", prd))
        orphan = sorted(mentioned - defined)
        if orphan:
            errors.append("RFs cited but not defined in the index: " + ", ".join("RF-" + x for x in orphan))
        notes.append(f"{len(defined)} RFs defined, {len(mentioned)} cited, 0 orphans" if not orphan else "incomplete RF index")

    # 5. the baseline SHA is still in the history (or in the squash) --------
    sha = str(ev.get("head_sha", ""))
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        errors.append(f"head_sha is not a full SHA: {sha!r}")
    elif live_head and not head_sha_matches_history(root, sha, live_head):
        errors.append(
            f"head_sha {sha[:12]} is not HEAD, an ancestor, nor tree-equivalent to "
            f"{live_head[:12]}: the baseline was measured over another tree"
        )
    if not live_head:
        errors.append("could not determine the live HEAD (git or MIGRATION_HEAD_SHA)")

    # 6. the decisions are re-derived from the living policies --------------
    live = quick_facts(root)
    missing = [f for f in ("Cargo.toml", "rust-toolchain.toml",
                           ".github/support.json", "README.md")
               if not (root / f).is_file()]
    if missing:
        errors.append("missing decision sources: " + ", ".join(missing))
    sup = support_facts(root)
    miri = miri_facts(root)
    derived = derive_decisions(live, edition_facts(root), sup, miri)

    drift = []
    for dotted in ev.get("decision_facts", DECISION_FACTS):
        try:
            frozen = get_path(ev, dotted)
        except KeyError:
            errors.append(f"decision_fact '{dotted}' does not exist in the baseline")
            continue
        current = live.get(dotted)
        if frozen != current:
            drift.append(f"{dotted}: {frozen!r} -> {current!r}")
    for key, value in derived.items():
        if ev.get("decisions", {}).get(key) != value:
            drift.append(f"decisions.{key}: {ev.get('decisions', {}).get(key)!r} -> {value!r}")

    if drift:
        # The pointer names the document and its section, and copies no command:
        # instructions in a message would be a second copy that rots. The reader
        # lands on the procedure itself (docs/src/release-process.md).
        msg = ("DECISION DRIFT; re-run --write, update §7 and re-approve "
               '(see docs/src/release-process.md, "Re-freezing the evidence baseline '
               'on a Release PR"): '
               + " | ".join(drift))
        if train_marker.is_file():
            errors.append(msg)
        else:
            notes.append("NOTE (train closed, non-blocking): " + msg)
    else:
        notes.append("decisions coherent with Cargo.toml, support.json, README and the toolchain")
    notes.append("train " + ("OPEN (drift is blocking)" if train_marker.is_file() else "closed (drift = NOTE)"))

    return (1 if errors else 0), errors, notes, ev


def main() -> int:
    here = Path(__file__).resolve().parent
    root_default = here.parent
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--root", type=Path, default=None, help="root of the repository to audit")
    ap.add_argument("--head-sha", default=None, help="override of the live HEAD")
    ap.add_argument("--write", type=Path, default=None, metavar="FILE")
    ap.add_argument("--check", type=Path, default=None, metavar="FILE")
    ap.add_argument("--prd", type=Path, default=None)
    ap.add_argument("--marker", type=Path, default=None)
    ap.add_argument("--json", action="store_true", help="--check: print a JSON summary")
    ap.add_argument(
        "--allow-unmerged-anchor", action="store_true",
        help="--write: accept a head_sha that is not on the default branch yet (a "
             "Release PR branch before the squash). Without this flag the freeze "
             "refuses, because that anchor breaks on the next PR",
    )
    args = ap.parse_args()

    import os
    root = (args.root or Path(os.environ.get("MIGRATION_ROOT", root_default))).resolve()
    explicit_sha = args.head_sha or os.environ.get("MIGRATION_HEAD_SHA") or None
    head_sha = resolve_head_sha(root, explicit_sha)

    if args.write:
        state = repository_state(root)
        if state == "nested":
            print(
                "::error:: --write refuses: the audit root is inside a repository but "
                "is not its top; run the freeze from the repository root",
                file=sys.stderr,
            )
            return 1
        if not head_sha:
            print("::error:: could not determine head_sha (git or --head-sha)", file=sys.stderr)
            return 1
        # Without a repository at all there is nothing to prove: the no-repo
        # fixture is legitimate and the --check guard already reports it as
        # "measured over another tree", so --write is not the place to block it.
        hazard = freeze_hazard(root, head_sha) if state == "top" else None
        if hazard and not args.allow_unmerged_anchor:
            print(f"::error:: {hazard}", file=sys.stderr)
            print(
                "  freeze the release commit that is ALREADY on the default branch, or pass "
                "--allow-unmerged-anchor and re-anchor after the squash",
                file=sys.stderr,
            )
            return 1
        if hazard:
            print(f"NOTE: {hazard} (accepted by --allow-unmerged-anchor)", file=sys.stderr)
        data = collect(root, head_sha)
        args.write.parent.mkdir(parents=True, exist_ok=True)
        args.write.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        print(f"OK: baseline written to {args.write} (head={head_sha[:12]}, "
              f"workspace={data['workspace_version']}, major={data['decisions']['major_target']}, "
              f"backports={data['decisions']['backport_line']})")
        return 0

    if args.check:
        ev_path = args.check if args.check.is_absolute() else root / args.check
        prd_rel = args.prd or Path(os.environ.get("MIGRATION_PRD", DEFAULT_PRD))
        prd_path = prd_rel if prd_rel.is_absolute() else root / prd_rel
        marker_rel = args.marker or Path(os.environ.get("MIGRATION_TRAIN_OPEN", DEFAULT_MARKER))
        marker = marker_rel if marker_rel.is_absolute() else root / marker_rel
        rc, errors, notes, ev = verify(root, ev_path, prd_path, marker, head_sha)
        if args.json:
            print(json.dumps({"ok": rc == 0, "errors": errors, "notes": notes,
                              "head_sha": ev.get("head_sha", "")}, ensure_ascii=False, indent=2))
        else:
            for line in errors:
                print("::error:: " + line)
            for line in notes:
                print("OK: " + line)
            if rc == 0:
                print("OK: migration evidence coherent with the tree")
        return rc

    ap.error("specify --write FILE or --check FILE")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
