#!/usr/bin/env python3
"""Re-auditoría reproducible de PRD-MIG-2026-01 y verificación fail-closed.

Por qué existe
--------------
El PRD v1 medía ~60 hechos contra un SHA y los copiaba en prosa. En este repo
se publican varias versiones por día: entre el baseline del v1 y hoy el
workspace pasó de 7.7.1 a 8.0.0, 8.0.1 y 8.0.2. La propia regla del v1
("si el repo se mueve, se re-corre el script") se activó y nadie la ejecutó.
Re-correr el script a mano NO arregla eso: la auditoría siguiente también es
una foto, y también envejece.

La corrección estructural es quitarle los hechos al documento:

  * `--write`  re-deriva TODOS los hechos del árbol y los congela en
               evidence/mig-2026-01/baseline.json (evidencia de F0).
  * `--check`  verifica que el PRD v2 no contenga hechos literales, que cada
               {{BASELINE.ruta}} resuelva, que no cite RFs inexistentes, que
               el SHA siga siendo HEAD, ancestro o el árbol de un squash, y
               que las DECISIONES que el v1 escribía a mano (major, línea de
               backport, gate de Miri, flip de edición) sigan siendo las que
               se DERIVAN de las políticas vivas del repo.

`--check` nunca compara un contador contra un número congelado: los contadores
son evidencia de F0, no un gate sobre main. Sí compara un conjunto pequeño de
hechos que *invalidan decisiones* (versión, canal, líneas de soporte, política
Miri, edición). Con `TRAIN_OPEN` presente esa deriva es bloqueante; sin el
marcador degrada a NOTE para no poner rojo el repo cuando el tren está cerrado.

Uso
---
  python3 scripts/audit_migration_baseline.py --write evidence/mig-2026-01/baseline.json
  python3 scripts/audit_migration_baseline.py --check evidence/mig-2026-01/baseline.json
  python3 scripts/audit_migration_baseline.py --check ... --json   # resumen

No requiere dependencias fuera de la stdlib, no escribe en el repo salvo en
`--write`, y no necesita cargo/rustc (importante: este sandbox no los tiene).
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

# Hechos que, si cambian, invalidan una DECISIÓN del PRD (no un contador).
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

# Cifras/frases del v1 que NO pueden volver a escribirse en el v2. El v2 las
# expresa como {{BASELINE.*}} o como decisión derivada. Es una lista de regex.
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
# utilidades de lectura (defensivas: un fichero ausente es un dato, no un crash)
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
    """Ejecuta git y conserva el código de salida (fail-closed en el caller)."""
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
    """Acepta HEAD, ancestría real o un SQUASH que preservó el árbol medido.

    La cola de merge usa SQUASH: el tip del PR no es ancestro de main, pero su
    árbol sí sobrevive al squash. `--write` congela el tip YA medido y recién
    después escribe baseline.json, así que entre el árbol del SHA y el del
    squash el único cambio legítimo es el propio baseline. Cualquier otra
    diferencia (o un git sin ese objeto) sigue fail-closed.
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


# Refs that survive a squash merge. The queue uses SQUASH (.github/merge-queue.json),
# so the tip of a Release PR is NOT in the history of main and never will be: freezing
# it passes the ancestry check exactly once, on the squash commit, and then fails on the
# first PR that touches any other file. Measured on #2909/#2914: the branch tip 4efcaf66
# was orphaned the moment the queue squashed it, and every later PR read as
# "el baseline fue medido sobre otro árbol" with no drift behind it.
DURABLE_REFS = ("main", "origin/main")


def durable_anchor(root: Path, sha: str) -> str | None:
    """Devuelve el primer ref durable del que `sha` es ancestro, o None."""
    for ref in DURABLE_REFS:
        rc, _ = git_rc(root, "merge-base", "--is-ancestor", sha, ref)
        if rc == 0:
            return ref
    return None


def freeze_hazard(root: Path, sha: str) -> str | None:
    """Diagnóstico de un SHA congelado que el próximo squash va a dejar huérfano.

    Devuelve None cuando el ancla es durable. Un SHA sin ref durable es aceptable
    SOLO en una rama cuyo tip todavía no está en main (el PR de release antes del
    squash): ahí el operador lo re-ancla a mano, con --allow-unmerged-anchor, y el
    gate lo vuelve a verificar en cada PR posterior.
    """
    if durable_anchor(root, sha):
        return None
    rc, _ = git_rc(root, "rev-parse", "--verify", "--quiet", sha)
    if rc != 0:
        return f"head_sha {sha[:12]} no es un objeto de este repositorio"
    return (
        f"head_sha {sha[:12]} no es ancestro de {' ni '.join(DURABLE_REFS)}: "
        "el próximo squash lo dejará huérfano y el baseline fallará en el primer PR "
        "que toque otro archivo"
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
# derivación de hechos
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
        # Cuenta TODAS las apariciones, incluidas las citas parciales en
        # comentarios: el v1 fijó "35" en un acceptance y el árbol ya medía 37.
        references += len(re.findall(r"dtolnay/rust-toolchain@", text))
        # [ \t] y no \s: \s cruza saltos de línea y confunde el job id
        # `toolchain:` (ci.yml) con un pin, capturando `name:` de la línea
        # siguiente. Un job id no es un pin.
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
    # Sólo el array `members`: leer todo el bloque [workspace] mezcla
    # resolver = "2" y los `exclude` (fuzz, tools/...) con los miembros, y
    # haría creer que hay overrides de edición por crate donde no los hay.
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
            # esos manifiestos declaran edition antes de [dependencies]
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
    """Los 4 ficheros de los que dependen las decisiones. Barato para CI."""
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
    """RF-23/RF-24/RF-13/K9 dejan de ser números escritos a mano."""
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
# verificación fail-closed
# --------------------------------------------------------------------------
def verify(root: Path, evidence_path: Path, prd_path: Path,
           train_marker: Path, live_head: str) -> tuple[int, list[str], list[str], dict]:
    errors: list[str] = []
    notes: list[str] = []

    # 1. evidencia legible y completa -------------------------------------
    try:
        ev = json.loads(evidence_path.read_text(encoding="utf-8"))
    except (OSError, ValueError) as exc:
        return 1, [f"baseline inaccesible o JSON inválido: {exc}"], [], {}
    if ev.get("schema") != SCHEMA:
        errors.append(f"schema={ev.get('schema')!r}, se esperaba {SCHEMA}")
    for key in ("head_sha", "decisions", "decision_facts", "banned_literals",
                "workspace_version", "toolchain", "support", "miri", "edition"):
        if key not in ev:
            errors.append(f"falta la clave obligatoria '{key}' en el baseline")

    # 2. el PRD no puede contener hechos literales ------------------------
    prd = ""
    if not prd_path.is_file():
        errors.append(f"no se encontró el PRD v2 en {prd_path}")
    else:
        prd = prd_path.read_text(encoding="utf-8", errors="replace")
        for literal in ev.get("banned_literals", []):
            try:
                if re.search(literal, prd):
                    errors.append(f"el PRD v2 reintroduce el hecho literal /{literal}/")
            except re.error as exc:
                errors.append(f"regex inválida en banned_literals: {literal}: {exc}")

        # 3. cada placeholder resuelve contra la evidencia -----------------
        placeholders = sorted(set(re.findall(r"\{\{BASELINE\.([A-Za-z0-9_.]+)\}\}", prd)))
        for dotted in placeholders:
            try:
                get_path(ev, dotted)
            except KeyError:
                errors.append(f"placeholder {{{{BASELINE.{dotted}}}}} no resuelve en baseline.json")
        notes.append(f"{len(placeholders)} placeholders resueltos")

        # 4. deriva de numeración §3 -> §6: ningún RF huérfano --------------
        defined = set(re.findall(r"\*\*RF-(\d{2})", prd))
        mentioned = set(re.findall(r"\bRF-(\d{2})\b", prd))
        orphan = sorted(mentioned - defined)
        if orphan:
            errors.append("RFs citados pero no definidos en el índice: " + ", ".join("RF-" + x for x in orphan))
        notes.append(f"{len(defined)} RFs definidos, {len(mentioned)} citados, 0 huérfanos" if not orphan else "índice de RFs incompleto")

    # 5. el SHA del baseline sigue en la historia (o en el squash) ---------
    sha = str(ev.get("head_sha", ""))
    if not re.fullmatch(r"[0-9a-f]{40}", sha):
        errors.append(f"head_sha no es un SHA completo: {sha!r}")
    elif live_head and not head_sha_matches_history(root, sha, live_head):
        errors.append(
            f"head_sha {sha[:12]} no es HEAD, ancestro ni equivalente en árbol a "
            f"{live_head[:12]}: el baseline fue medido sobre otro árbol"
        )
    if not live_head:
        errors.append("no se pudo determinar el HEAD vivo (git o MIGRATION_HEAD_SHA)")

    # 6. las decisiones se re-derivan de las políticas vivas ---------------
    live = quick_facts(root)
    missing = [f for f in ("Cargo.toml", "rust-toolchain.toml",
                           ".github/support.json", "README.md")
               if not (root / f).is_file()]
    if missing:
        errors.append("faltan fuentes de decisión: " + ", ".join(missing))
    sup = support_facts(root)
    miri = miri_facts(root)
    derived = derive_decisions(live, edition_facts(root), sup, miri)

    drift = []
    for dotted in ev.get("decision_facts", DECISION_FACTS):
        try:
            frozen = get_path(ev, dotted)
        except KeyError:
            errors.append(f"decision_fact '{dotted}' no existe en el baseline")
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
        msg = ("DERIVA DE DECISIÓN; re-corre --write, actualiza §7 y re-aprueba "
               "(ver docs/src/release-process.md, «Re-freezing the evidence baseline "
               "on a Release PR»): "
               + " | ".join(drift))
        if train_marker.is_file():
            errors.append(msg)
        else:
            notes.append("NOTE (tren cerrado, no bloqueante): " + msg)
    else:
        notes.append("decisiones coherentes con Cargo.toml, support.json, README y toolchain")
    notes.append("tren " + ("ABIERTO (deriva bloqueante)" if train_marker.is_file() else "cerrado (deriva = NOTE)"))

    return (1 if errors else 0), errors, notes, ev


def main() -> int:
    here = Path(__file__).resolve().parent
    root_default = here.parent
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--root", type=Path, default=None, help="raíz del repo a auditar")
    ap.add_argument("--head-sha", default=None, help="override del HEAD vivo")
    ap.add_argument("--write", type=Path, default=None, metavar="FILE")
    ap.add_argument("--check", type=Path, default=None, metavar="FILE")
    ap.add_argument("--prd", type=Path, default=None)
    ap.add_argument("--marker", type=Path, default=None)
    ap.add_argument("--json", action="store_true", help="--check: imprime resumen JSON")
    ap.add_argument(
        "--allow-unmerged-anchor", action="store_true",
        help="--write: acepta un head_sha que todavía no está en main (rama de un PR de "
             "release antes del squash). Sin este flag el freeze se niega, porque ese ancla "
             "se rompe en el PR siguiente",
    )
    args = ap.parse_args()

    import os
    root = (args.root or Path(os.environ.get("MIGRATION_ROOT", root_default))).resolve()
    explicit_sha = args.head_sha or os.environ.get("MIGRATION_HEAD_SHA") or None
    head_sha = resolve_head_sha(root, explicit_sha)

    if args.write:
        if not head_sha:
            print("::error:: no se pudo determinar head_sha (git o --head-sha)", file=sys.stderr)
            return 1
        hazard = freeze_hazard(root, head_sha)
        # Un árbol sin .git no puede probar ancestría: el guard de --check ya lo
        # reporta como "medido sobre otro árbol" en los fixtures sin repo, así que
        # --write no es el lugar para bloquearlo.
        has_git = (root / ".git").exists()
        if hazard and has_git and not args.allow_unmerged_anchor:
            print(f"::error:: {hazard}", file=sys.stderr)
            print(
                "  congelá el commit del release que YA está en main, o pasá "
                "--allow-unmerged-anchor y re-anclá después del squash",
                file=sys.stderr,
            )
            return 1
        if hazard:
            print(f"NOTE: {hazard} (aceptado por --allow-unmerged-anchor)", file=sys.stderr)
        data = collect(root, head_sha)
        args.write.parent.mkdir(parents=True, exist_ok=True)
        args.write.write_text(json.dumps(data, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
        print(f"OK: baseline escrito en {args.write} (head={head_sha[:12]}, "
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
                print("OK: evidencia de migración coherente con el árbol")
        return rc

    ap.error("indicá --write FILE o --check FILE")
    return 2


if __name__ == "__main__":
    raise SystemExit(main())
