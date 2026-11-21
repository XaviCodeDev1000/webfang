#!/usr/bin/env bash
# Semantics harness for scripts/check_migration_evidence.sh — issue #2056.
#
# Hermético: construye árboles mínimos en mktemp, no llama a cargo, rustc, red
# ni servicios externos, y nunca toca el árbol real. Dos casos crean un repo
# git LOCAL con fechas fijas para reproducir un SQUASH. Cada caso negativo
# lleva su `# NEGATION:`.
# El caso límite (tren cerrado) es un CONTROL POSITIVO y por eso NO lo lleva:
# el guard debe dejar pasar la deriva cuando TRAIN_OPEN no existe.
#
# El meta-guard reemplaza el guard por `exit 0` y vuelve a correr este harness:
# todos los casos negativos pasan entonces, así que el harness VA A ROJO. Eso es
# lo que prueba que el guard decide algo y no es decoración.
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
GUARD="${MIGRATION_GUARD:-$ROOT/scripts/check_migration_evidence.sh}"
AUDITOR="$ROOT/scripts/audit_migration_baseline.py"
SHA="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

PASS=0
FAIL=0
ok()  { PASS=$((PASS + 1)); printf 'OK: %s\n' "$1"; }
bad() { FAIL=$((FAIL + 1)); printf 'FAIL: %s\n' "$1"; }

fixture() { # -> imprime la ruta de un árbol coherente con baseline.json
  local d
  d="$(mktemp -d "$WORK/tree.XXXXXX")"
  mkdir -p "$d/.github/workflows" "$d/evidence/mig-2026-01" \
           "$d/crates/webfang_test_utils" "$d/crates/webfang_core"
  cat > "$d/Cargo.toml" <<'TOML'
[workspace]
resolver = "2"
members = ["crates/webfang_test_utils", "crates/webfang_core"]

[workspace.package]
version = "8.0.2"
edition = "2021"
rust-version = "1.91"

[workspace.lints.rust]
unsafe_code = "deny"
TOML
  printf 'edition.workspace = true\n' > "$d/crates/webfang_test_utils/Cargo.toml"
  printf 'edition.workspace = true\n' > "$d/crates/webfang_core/Cargo.toml"
  cat > "$d/rust-toolchain.toml" <<'TOML'
[toolchain]
channel = "1.91.1"
components = ["rustfmt", "clippy"]
TOML
  cat > "$d/.github/support.json" <<'JSON'
{"schema_version":1,"lines":[
 {"minor":"8.0","state":"STABLE","latest":"8.0.2","accepts":["bugfix","security"]},
 {"minor":"7.7","state":"MAINTENANCE","latest":"7.7.1","accepts":["security"]},
 {"minor":"7.6","state":"EOL","latest":"7.6.6","eol_reason":"auto: rotate-stable, third line back from STABLE"}
]}
JSON
  cat > "$d/README.md" <<'MD'
WebFang is a **binaries-only** distribution. Nothing is published to crates.io.
**Miri status:** the Miri lanes are **advisory**: scheduled-only, continue-on-error, and never a merge gate.
MD
  cat > "$d/.github/workflows/ci.yml" <<'YML'
jobs:
  miri:
    continue-on-error: true
  miri-infra-fast: {}
  miri-infra-slow: {}
YML
  cat > "$d/.github/workflows/release.yml" <<'YML'
matrix:
  include:
    - target: x86_64-unknown-linux-gnu
YML
  python3 "$AUDITOR" --root "$d" --head-sha "$SHA" \
    --write "$d/evidence/mig-2026-01/baseline.json" >/dev/null 2>&1 \
    || { printf 'fixture: --write falló\n' >&2; return 1; }
  cat > "$d/evidence/mig-2026-01/PRD-v2.md" <<'MD'
# PRD fixture
Workspace {{BASELINE.workspace_version}} en {{BASELINE.head_sha}}.
Major {{BASELINE.decisions.major_target}}, backports {{BASELINE.decisions.backport_line}}.
Índice: **RF-01** congelar baseline; **RF-23** semver; **RF-24** backports.
MD
  : > "$d/evidence/mig-2026-01/TRAIN_OPEN"
  printf '%s' "$d"
}

run_guard() { # tree [head_sha]
  local d="$1" sha="${2:-$SHA}"
  MIGRATION_ROOT="$d" \
  MIGRATION_AUDITOR="$AUDITOR" \
  MIGRATION_EVIDENCE="$d/evidence/mig-2026-01/baseline.json" \
  MIGRATION_PRD="$d/evidence/mig-2026-01/PRD-v2.md" \
  MIGRATION_TRAIN_OPEN="$d/evidence/mig-2026-01/TRAIN_OPEN" \
  MIGRATION_HEAD_SHA="$sha" \
    bash "$GUARD" >"$WORK/out.log" 2>&1
  return $?
}

expect_ok()  { local label="$1"; shift; if run_guard "$@"; then ok "$label"; else bad "$label (esperaba verde)"; sed 's/^/    /' "$WORK/out.log"; fi; }
expect_red() { local label="$1"; shift; if run_guard "$@"; then bad "$label (esperaba rojo, guard verde)"; sed 's/^/    /' "$WORK/out.log"; else ok "$label"; fi; }

# Identidad y fecha deterministas: el runner de CI no tiene user.name/email
# global, así que TODO commit del harness debe llevarla explícita.
git_identity() {
  export GIT_AUTHOR_DATE="2026-01-01T00:00:00Z" GIT_COMMITTER_DATE="2026-01-01T00:00:00Z"
  export GIT_AUTHOR_NAME="Migration Test" GIT_AUTHOR_EMAIL="migration@example.com"
  export GIT_COMMITTER_NAME="Migration Test" GIT_COMMITTER_EMAIL="migration@example.com"
}

# Imprime "<sha_medido> <sha_squash>": base -> PR -> tip con baseline, y un
# squash que comparte el ÁRBOL del tip pero cuyo padre es la base. Así el SHA
# medido NO es ancestro del squash y el guard debe decidir por árbol, igual que
# hace la cola de merge de este repo (.github/merge-queue.json, SQUASH).
make_squash_tree() { # dir -> "<measured_sha> <squash_sha>"
  local d="$1" base measured tree
  (
    cd "$d" || exit 1
    git_identity
    git init -q
    git add -A && git commit -q -m base
    base="$(git rev-parse HEAD)"
    printf 'release work\n' > notes.txt
    git add -A && git commit -q -m pr-work
    measured="$(git rev-parse HEAD)"
    # Este fixture representa el estado PRE-squash: el commit medido todavía no
    # está en un ref durable, que es justo lo que el agregado de release produce.
    # --allow-unmerged-anchor lo dice explícitamente; sin el flag --write se
    # niega, porque congelar un ancla así la deja huérfana en el PR siguiente
    # (#2909 -> 4efcaf66 quedó fuera de main para siempre).
    python3 "$AUDITOR" --root "$d" --write "$d/evidence/mig-2026-01/baseline.json" \
      --allow-unmerged-anchor >/dev/null 2>&1 || exit 1
    git add -A && git commit -q -m tip
    tree="$(git rev-parse 'HEAD^{tree}')"
    printf '%s %s\n' "$measured" "$(git commit-tree "$tree" -p "$base" -m squash)"
  )
}

expect_red_message() { # label tree head_sha expected_message
  local label="$1" tree="$2" sha="$3" msg="$4"
  if run_guard "$tree" "$sha"; then
    bad "$label (esperaba rojo, guard verde)"
    sed 's/^/    /' "$WORK/out.log"
  elif grep -Fq "$msg" "$WORK/out.log"; then
    ok "$label"
  else
    bad "$label (rojo pero sin el mensaje esperado)"
    sed 's/^/    /' "$WORK/out.log"
  fi
}

# --- 1. control positivo: un árbol coherente debe estar verde ---------------
D="$(fixture)" || { echo "FAIL: no se pudo construir el fixture"; exit 1; }
expect_ok "baseline + PRD + decisiones coherentes -> verde" "$D"

# --- 2. evidencia ausente ---------------------------------------------------
# NEGATION: un baseline borrado no puede degradar a "sin evidencia, sigo".
D2="$(fixture)"; rm -f "$D2/evidence/mig-2026-01/baseline.json"
expect_red "baseline ausente -> rojo" "$D2"

# --- 3. JSON corrupto -------------------------------------------------------
# NEGATION: un baseline ilegible no es un baseline vacío.
D3="$(fixture)"; printf '{' > "$D3/evidence/mig-2026-01/baseline.json"
expect_red "baseline con JSON inválido -> rojo" "$D3"

# --- 4. hecho literal reintroducido en la prosa -----------------------------
# NEGATION: el v1 murió de copiar cifras en el documento; el v2 las prohíbe.
D4="$(fixture)"; printf '\nv7.7.1\n' >> "$D4/evidence/mig-2026-01/PRD-v2.md"
expect_red "hecho literal del v1 en el PRD -> rojo" "$D4"

# --- 5. placeholder sin resolver --------------------------------------------
# NEGATION: un placeholder inventado debe fallar, no imprimirse tal cual.
D5="$(fixture)"; printf '\n{{BASELINE.no.existe.esta.clave}}\n' >> "$D5/evidence/mig-2026-01/PRD-v2.md"
expect_red "placeholder huérfano -> rojo" "$D5"

# --- 6. RF huérfano (la deriva §3 -> §6 del v1) -----------------------------
# NEGATION: C14 citaba un requisito que no existía; el índice lo detecta.
D6="$(fixture)"; printf '\nVer RF-99 para guardas de ruta.\n' >> "$D6/evidence/mig-2026-01/PRD-v2.md"
expect_red "RF citado y no definido -> rojo" "$D6"

# --- 7. deriva de una decisión tras un release ------------------------------
# NEGATION: un release que rota support.json invalida RF-23/RF-24 en silencio.
D7="$(fixture)"; sed -i 's/version = "8.0.2"/version = "9.0.0"/' "$D7/Cargo.toml"
expect_red "versión del workspace derivada -> rojo con tren abierto" "$D7"

# --- 8. CONTROL POSITIVO: tren cerrado, la deriva es NOTE -------------------
# Sin # NEGATION: este caso DEBE pasar. Es el límite que impide un gate ruidoso.
D8="$(fixture)"; sed -i 's/version = "8.0.2"/version = "9.0.0"/' "$D8/Cargo.toml"
rm -f "$D8/evidence/mig-2026-01/TRAIN_OPEN"
expect_ok "tren cerrado + deriva -> NOTE no bloqueante" "$D8"

# --- 9. decisión congelada que ya no se deriva de la política ---------------
# NEGATION: si support.json rota, major/backport se recalculan; un JSON viejo
# con las decisiones pegadas a mano debe caer.
D9="$(fixture)"; python3 - "$D9/evidence/mig-2026-01/baseline.json" <<'PY'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
d["decisions"]["major_target"] = "1.0.0"
json.dump(d, open(p, "w"))
PY
expect_red "decisiones que no se derivan de support.json -> rojo" "$D9"

# --- 10. baseline medido sobre otro árbol -----------------------------------
# NEGATION: el SHA del baseline no puede ser cualquier cadena de 40 hex.
D10="$(fixture)"
expect_red "head_sha que no es HEAD ni ancestro -> rojo" "$D10" "0000000000000000000000000000000000000000"

# --- 11. CONTROL POSITIVO: el tip medido sobrevive al SQUASH ----------------
# Sin # NEGATION: el guard DEBE pasar cuando el baseline se midió en el tip del
# PR y el squash conservó su árbol cambiando sólo baseline.json.
D11="$(fixture)"
read -r measured squash <<<"$(make_squash_tree "$D11")"
if git -C "$D11" merge-base --is-ancestor "$measured" "$squash"; then
  bad "fixture squash inválida: el SHA medido sí es ancestro (no prueba tree-equality)"
else
  expect_ok "tip medido + squash del mismo árbol -> verde" "$D11" "$squash"
fi

# --- 12. baseline medido sobre OTRO ÁRBOL -----------------------------------
# NEGATION: un squash con otro árbol no puede colar aunque el SHA sea real.
D12="$(fixture)"
read -r measured squash <<<"$(make_squash_tree "$D12")"
(
  cd "$D12" || exit 1
  git_identity
  printf 'otro arbol\n' > other.txt
  git add -A && git commit -q -m other
)
other="$(git -C "$D12" rev-parse HEAD)"
python3 - "$D12/evidence/mig-2026-01/baseline.json" "$other" <<'PY'
import json, sys
p, sha = sys.argv[1], sys.argv[2]
d = json.load(open(p))
d["head_sha"] = sha
json.dump(d, open(p, "w"))
PY
expect_red_message "SHA de otro árbol -> rojo con mensaje de árbol" "$D12" "$squash" "el baseline fue medido sobre otro árbol"

# --- 13. SHA inexistente con git real ---------------------------------------
# NEGATION: un objeto que no existe no puede colar por la rama de árbol.
D13="$(fixture)"
read -r measured squash <<<"$(make_squash_tree "$D13")"
python3 - "$D13/evidence/mig-2026-01/baseline.json" <<'PY'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
d["head_sha"] = "f" * 40
json.dump(d, open(p, "w"))
PY
expect_red_message "head_sha inexistente con git real -> rojo con mensaje" "$D13" "$squash" "el baseline fue medido sobre otro árbol"

# --- 14. repo sin .git -------------------------------------------------------
# NEGATION: sin git no se puede probar ancestría ni equivalencia de árbol.
D14="$(fixture)"
expect_red_message "repo sin .git -> rojo con mensaje de árbol" "$D14" "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" "el baseline fue medido sobre otro árbol"

# --- 15. --write se niega a congelar un ancla huérfana ---------------------
# NEGATION: es el defecto de #2909. `--write` congeló el tip de la rama del
# release; la cola lo squasheó y el commit quedó fuera de main para siempre, así
# que el baseline pasó una vez y falló en el PR siguiente. El guard tiene que
# negarse a escribir ese ancla salvo que el operador lo pida explícitamente.
D15="$(fixture)"
(
  cd "$D15" || exit 1
  git_identity
  git init -q
  git add -A && git commit -q -m base
  git branch -M main
  printf 'orphan\n' > orphan.txt
  git add -A && git commit -q -m "tip fuera de main"
  git checkout -q -b release-line 2>/dev/null || true
  printf 'release\n' > rel.txt
  git add -A && git commit -q -m "tip de la rama del release"
) >/dev/null 2>&1
ORPHAN_SHA="$(git -C "$D15" rev-parse HEAD)"
if [ "$ORPHAN_SHA" = "$(git -C "$D15" rev-parse main)" ]; then
  bad "fixture 15 inválida: el tip no puede coincidir con main"
else
  if python3 "$AUDITOR" --root "$D15" --head-sha "$ORPHAN_SHA" \
       --write "$WORK/orphan.json" >"$WORK/w.log" 2>&1; then
    bad "--write aceptó un ancla huérfana (regresión de #2909)"
    sed 's/^/    /' "$WORK/w.log"
  elif grep -Fq "the next squash will orphan it" "$WORK/w.log"; then
    ok "--write rechaza un ancla que ningún ref durable alcanza"
  else
    bad "--write falló pero sin el mensaje esperado"
    sed 's/^/    /' "$WORK/w.log"
  fi
  # El escape explícito tiene que seguir existiendo para el estado pre-squash.
  if python3 "$AUDITOR" --root "$D15" --head-sha "$ORPHAN_SHA" \
       --write "$WORK/orphan.json" --allow-unmerged-anchor >"$WORK/w2.log" 2>&1; then
    ok "--allow-unmerged-anchor sigue aceptando el estado pre-squash"
  else
    bad "--allow-unmerged-anchor no aceptó el estado pre-squash"
    sed 's/^/    /' "$WORK/w2.log"
  fi
fi

# --- 16. default branch is not main -----------------------------------------
# NEGATION: the durable refs are the default branch, local and remote, detected
# through origin's HEAD symref. A repository whose default branch is `develop`
# must still prove ancestry; treating a missing `main` as "not an ancestor"
# would block every valid --write there.
D16="$(mktemp -d "$WORK/nonmain.XXXXXX")"
(
  cd "$D16" || exit 1
  git_identity
  git init -q -b develop
  printf 'measured facts\n' > facts.txt
  git add -A && git commit -q -m base
  git init -q --bare "$D16.git"
  git remote add origin "$D16.git"
  git push -q origin develop
  git fetch -q origin
  git symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/develop
) >/dev/null 2>&1
NONMAIN_SHA="$(git -C "$D16" rev-parse HEAD)"
if python3 "$AUDITOR" --root "$D16" --head-sha "$NONMAIN_SHA" \
     --write "$D16/evidence/baseline.json" >"$WORK/w3.log" 2>&1; then
  ok "--write proves ancestry through origin/HEAD when main does not exist"
else
  bad "--write refused a durable anchor on a non-main default branch"
  sed 's/^/    /' "$WORK/w3.log"
fi

# --- 17. no durable ref exists in the repository ----------------------------
# NEGATION: without origin (and without main) ancestry cannot be proven. The
# refusal must say so, instead of claiming the commit is "not an ancestor" of
# refs that do not exist here. The explicit escape (--allow-unmerged-anchor)
# keeps serving the pre-squash state.
D17="$(mktemp -d "$WORK/norefs.XXXXXX")"
(
  cd "$D17" || exit 1
  git_identity
  git init -q -b develop
  printf 'measured facts\n' > facts.txt
  git add -A && git commit -q -m base
) >/dev/null 2>&1
NOREFS_SHA="$(git -C "$D17" rev-parse HEAD)"
if python3 "$AUDITOR" --root "$D17" --head-sha "$NOREFS_SHA" \
     --write "$D17/evidence/baseline.json" >"$WORK/w4.log" 2>&1; then
  bad "--write accepted an anchor with no durable ref in the repository"
elif grep -Fq "no durable ref exists in this repository" "$WORK/w4.log"; then
  ok "--write reports missing durable refs instead of a fake orphan claim"
else
  bad "--write failed without the no-durable-ref message"
  sed 's/^/    /' "$WORK/w4.log"
fi

# --- 18. --write root is a repository subdirectory --------------------------
# NEGATION: a root without .git at its top is ambiguous — the no-repo fixture
# is legitimate, but the same shape is reachable in production by pointing
# --root at a subdirectory, where the hazard check would be silently skipped.
# The freeze must tell "no repository at all" apart from "a repository I could
# not resolve" and fail closed on the latter.
D18="$(mktemp -d "$WORK/nested.XXXXXX")"
(
  cd "$D18" || exit 1
  git_identity
  git init -q -b main
  printf 'measured facts\n' > facts.txt
  git add -A && git commit -q -m base
) >/dev/null 2>&1
mkdir -p "$D18/sub/deeper"
NESTED_SHA="$(git -C "$D18" rev-parse HEAD)"
if python3 "$AUDITOR" --root "$D18/sub/deeper" --head-sha "$NESTED_SHA" \
     --write "$WORK/nested.json" >"$WORK/w5.log" 2>&1; then
  bad "--write accepted a repository subdirectory root (hazard check skipped)"
elif grep -Fq "is not its top" "$WORK/w5.log"; then
  ok "--write refuses a repository subdirectory root (fail closed)"
else
  bad "--write failed without the subdirectory refusal message"
  sed 's/^/    /' "$WORK/w5.log"
fi

printf '\nmigration evidence harness: %d OK, %d FAIL\n' "$PASS" "$FAIL"
[[ $FAIL -eq 0 ]] || exit 1
exit 0
