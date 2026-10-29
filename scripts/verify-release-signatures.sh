#!/usr/bin/env bash
# verify-release-signatures.sh — fail-closed keyless-signature verification for
# every asset this repository publishes (issue #1607, SC-15, SC-17).
#
# WHAT THIS IS FOR
# ================
# A `SHA256SUMS.txt` written by the same job that built the binaries is not
# evidence of anything: whoever could tamper with the archives could rewrite the
# list. The release pipeline now signs every published asset with cosign's
# KEYLESS flow (sigstore + the GitHub Actions OIDC issuer), so the checksum
# manifest is signed by an identity that is independent of the job that produced
# it. This script is the consumer side of that property, and the release's own
# gate: `publish` runs it BEFORE `gh release create`, so a release whose assets
# are unsigned, tampered, or signed by a foreign identity is never created.
#
# A MISSING OR INVALID BUNDLE BLOCKS THE RELEASE. That is the entire point, and
# it is why there is no "cannot verify, carry on" branch anywhere below:
#   * cosign not on PATH           -> FAIL (never a skip). An absent verifier
#                                     that reads as "nothing to check" is how an
#                                     unsigned release ships with a green job.
#   * bundle file missing          -> FAIL. An asset with no bundle is unsigned.
#   * bundle present, signature
#     invalid or identity foreign  -> FAIL. cosign's own non-zero exit.
#   * no assets passed at all      -> FAIL. An empty verification is not a pass.
#
# THE TRUST BOUNDARY, STATED HONESTLY
# ====================================
# Keyless means there is no long-lived signing key, so the signing identity is
# the WORKFLOW PATH, not a person: the certificate's SAN is
#     https://github.com/<owner>/<repo>/.github/workflows/release.yml@<REF>
# and the trust anchor is the public Rekor transparency log, with the
# certificate minted by the GitHub Actions OIDC issuer. Consequently:
#   * A consumer trusting `--certificate-identity-regexp` is trusting "this
#     workflow file, in this repository, signed during a run of that workflow".
#   * A consumer is NOT getting a person or an org signature, and this script
#     must never be described as if they were. The regex below is what pins the
#     boundary, and it is printed on every run so the value under test is
#     visible rather than implied.
#   * A long-lived key (cosign key-pair, KMS) would be strictly stronger for
#     consumers who want a stable identity, and needs a maintainer-held secret.
#     That secret cannot be provisioned from inside this repository, which is
#     why the keyless design is the one that can actually be landed.
#
# WHICH RUN, NOT JUST WHICH WORKFLOW (webfang#2464)
# =================================================
# <REF> in that SAN is whatever ref the workflow run was based on, taken from
# the ambient OIDC token: `refs/tags/vX.Y.Z` for a tag push, but
# `refs/heads/<default-branch>` for a `workflow_dispatch` — which is how every
# backstop and every reconciliation sweep runs. An identity regexp that admits
# only `@refs/tags/v...` therefore rejects the pipeline's own output on the one
# path that exists to recover a failed release, after every build, checksum,
# attestation and signature has already succeeded.
#
# The fix is NOT to widen the regexp until it matches both, because then the
# regexp no longer says anything about the ref. Instead:
#   * the identity regexp states WHICH WORKFLOW FILE (repo + path, ref left as
#     a shape), and
#   * --workflow-ref / --workflow-trigger state WHICH RUN, as exact equality
#     against the certificate's own OIDC claims
#     (--certificate-github-workflow-ref / --certificate-github-workflow-trigger,
#     documented in cosign v3.0.6).
# A regexp with a typo can silently widen; a claim mismatch cannot. Both are
# printed on every run.
#
# The default remains TAG-ONLY on purpose: a caller that pins no claims gets the
# strict historical behaviour, so forgetting the new flags can only fail closed.
#
# The residual, stated instead of hidden: a caller that supplies a ref-SHAPE
# identity (the `refs/(heads|tags)/` form) and NO claims has widened the gate to
# "some run of this workflow", because this script cannot tell that regexp from
# a deliberate one. Nothing inside this file prevents it. What prevents it is
# scripts/check_release_dispatch.sh, which fails the build if release.yml's
# identity loses either claim — so the workflow cannot reach that state without
# CI saying so.
#
# USAGE
#   bash scripts/verify-release-signatures.sh --help
#   bash scripts/verify-release-signatures.sh [--identity-regexp RE] <asset> [asset ...]
#   bash scripts/verify-release-signatures.sh --identity-regexp RE \
#       --workflow-ref refs/heads/main --workflow-trigger workflow_dispatch <asset> ...
#
# Each <asset> must have a sibling bundle named <asset>.sigstore.json, as
# produced by `cosign sign-blob --bundle <asset>.sigstore.json <asset>`.
#
# ENVIRONMENT OVERRIDES
#   RELEASE_CERT_IDENTITY_REGEXP    identity regexp (same effect as the flag)
#   RELEASE_CERT_WORKFLOW_REF       pinned ref claim (same as --workflow-ref)
#   RELEASE_CERT_WORKFLOW_TRIGGER   pinned trigger claim (same as --workflow-trigger)
#   COSIGN                          path to the cosign binary (default: cosign on PATH)
#
# STYLE: user-facing errors are Spanish (repo convention for this scripts/
# family); internal output is English.

set -euo pipefail

export LC_ALL=C

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
LEGACY_ALLOWLIST="$SCRIPT_DIR/release-legacy-tags.tsv"

# The default identity is the CURRENT repository, derived from GITHUB_REPOSITORY
# when Actions provides it (a fork verifies its own releases without editing
# this file). The fallback is the canonical repository. The release gate never
# uses the historical allowlist below; that path is explicit via --tag.
DEFAULT_REPO="${GITHUB_REPOSITORY:-XaviCodeDev1000/webfang}"
DEFAULT_REPO_ESCAPED="${DEFAULT_REPO//./\\.}"
DEFAULT_IDENTITY_REGEXP="^https://github\\.com/${DEFAULT_REPO_ESCAPED}/\\.github/workflows/release\\.yml@refs/tags/v[0-9].*$"

# The OIDC issuer. Pinned, not derived: a certificate from any other issuer is
# not an Actions keyless certificate.
OIDC_ISSUER='https://token.actions.githubusercontent.com'

usage() {
    cat <<'USAGE'
verify-release-signatures.sh — verify every published release asset against its
keyless (sigstore + GitHub OIDC) cosign signature bundle. Fail-closed: a missing
cosign binary, a missing bundle, an invalid signature or a foreign identity all
block the release.

usage:
  bash scripts/verify-release-signatures.sh --help
  bash scripts/verify-release-signatures.sh [--identity-regexp RE] <asset> [asset ...]
  bash scripts/verify-release-signatures.sh --identity-regexp RE \
      --workflow-ref REF --workflow-trigger EVENT <asset> [asset ...]

Each <asset> requires a sibling bundle named <asset>.sigstore.json, produced by
  cosign sign-blob --bundle <asset>.sigstore.json <asset>

options:
  --identity-regexp RE   Override the certificate identity regexp. When set,
                         the default and the historical allowlist are ignored.
                         Default: the release workflow of the current
                         repository (GITHUB_REPOSITORY, falling back to
                         XaviCodeDev1000/webfang) at a version tag.
  --workflow-ref REF     Pin the certificate's GitHub workflow ref claim
                         (refs/heads/... or refs/tags/...). Required to verify
                         a workflow_dispatch run, whose SAN carries the default
                         branch instead of a tag. Requires --identity-regexp
                         and --workflow-trigger.
  --workflow-trigger EV  Pin the certificate's GitHub workflow trigger claim
                         (push, workflow_dispatch, ...). Requires
                         --workflow-ref.
  --tag TAG              Verify a tag published before the repository
                         migration. TAG must be listed in
                         scripts/release-legacy-tags.tsv; new tags are
                         rejected. Cannot be combined with --identity-regexp,
                         RELEASE_CERT_IDENTITY_REGEXP, or claim pinning.
  -h, --help             Print this help and exit 0 (no cosign required).

environment:
  RELEASE_CERT_IDENTITY_REGEXP    same override as --identity-regexp
  RELEASE_CERT_WORKFLOW_REF       same as --workflow-ref
  RELEASE_CERT_WORKFLOW_TRIGGER   same as --workflow-trigger
  COSIGN                          path to the cosign binary (default: cosign)

exit codes:
  0  every asset verified against its bundle
  1  verification failed (missing tool, missing bundle, bad signature, foreign
     identity, or no assets given)
  2  usage error
USAGE
}

IDENTITY_REGEXP=""
IDENTITY_EXPLICIT=false
WORKFLOW_REF="${RELEASE_CERT_WORKFLOW_REF:-}"
WORKFLOW_TRIGGER="${RELEASE_CERT_WORKFLOW_TRIGGER:-}"
TAG=""
COSIGN_BIN="${COSIGN:-cosign}"
ASSETS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        -h|--help)
            usage
            exit 0
            ;;
        --identity-regexp)
            if [[ $# -lt 2 ]]; then
                echo "::error::--identity-regexp requiere un valor." >&2
                exit 2
            fi
            IDENTITY_REGEXP="$2"
            IDENTITY_EXPLICIT=true
            shift 2
            ;;
        --identity-regexp=*)
            IDENTITY_REGEXP="${1#--identity-regexp=}"
            IDENTITY_EXPLICIT=true
            shift
            ;;
        --tag)
            if [[ $# -lt 2 || -z "$2" ]]; then
                echo "::error::--tag requiere un valor." >&2
                exit 2
            fi
            TAG="$2"
            shift 2
            ;;
        --tag=*)
            TAG="${1#--tag=}"
            if [[ -z "$TAG" ]]; then
                echo "::error::--tag requiere un valor." >&2
                exit 2
            fi
            shift
            ;;
        --workflow-ref)
            if [[ $# -lt 2 ]]; then
                echo "::error::--workflow-ref requiere un valor." >&2
                exit 2
            fi
            WORKFLOW_REF="$2"
            shift 2
            ;;
        --workflow-ref=*)
            WORKFLOW_REF="${1#--workflow-ref=}"
            shift
            ;;
        --workflow-trigger)
            if [[ $# -lt 2 ]]; then
                echo "::error::--workflow-trigger requiere un valor." >&2
                exit 2
            fi
            WORKFLOW_TRIGGER="$2"
            shift 2
            ;;
        --workflow-trigger=*)
            WORKFLOW_TRIGGER="${1#--workflow-trigger=}"
            shift
            ;;
        --)
            shift
            while [[ $# -gt 0 ]]; do
                ASSETS+=("$1")
                shift
            done
            ;;
        -*)
            echo "::error::opción desconocida: $1" >&2
            usage >&2
            exit 2
            ;;
        *)
            ASSETS+=("$1")
            shift
            ;;
    esac
done

if [[ "$IDENTITY_EXPLICIT" == "true" && -z "$IDENTITY_REGEXP" ]]; then
    echo "::error::--identity-regexp requiere un valor no vacío." >&2
    exit 2
fi

if [[ -n "$TAG" ]]; then
    if [[ "$IDENTITY_EXPLICIT" == "true" || -n "${RELEASE_CERT_IDENTITY_REGEXP:-}" \
          || -n "$WORKFLOW_REF" || -n "$WORKFLOW_TRIGGER" ]]; then
        echo "::error::--tag no se puede combinar con --identity-regexp, con RELEASE_CERT_IDENTITY_REGEXP, ni con claim pinning (--workflow-ref / --workflow-trigger). La identidad histórica es de otra era: un tag pre-migración se publicó por tag push, así que su ref y su trigger no se negocian." >&2
        exit 2
    fi
    if [[ ! -f "$LEGACY_ALLOWLIST" ]]; then
        echo "::error::falta la allowlist histórica '$LEGACY_ALLOWLIST'; no se verifica una release histórica sin ella." >&2
        exit 1
    fi
    if ! LEGACY_REPO=$(awk -F '\t' -v tag="$TAG" '$1 == tag { print $2; found=1; exit } END { if (!found) exit 1 }' "$LEGACY_ALLOWLIST"); then
        echo "::error::el tag '$TAG' no está en la allowlist histórica; un tag nuevo no puede usar la identidad antigua." >&2
        exit 1
    fi
    LEGACY_REPO_ESCAPED="${LEGACY_REPO//./\\.}"
    IDENTITY_REGEXP="^https://github\\.com/${LEGACY_REPO_ESCAPED}/\\.github/workflows/release\\.yml@refs/tags/v[0-9].*$"
    echo "historical tag allowlist: $TAG"
fi

if [[ -z "$IDENTITY_REGEXP" ]]; then
    IDENTITY_REGEXP="${RELEASE_CERT_IDENTITY_REGEXP:-$DEFAULT_IDENTITY_REGEXP}"
fi

# ─────────────────────────────────────────────────────────────────────────────
# Claim pinning is all-or-nothing, and it needs an identity that does not
# contradict it (webfang#2464). Every branch below fails closed: a half pin, a
# typo'd ref, or claims paired with the tag-only default would all end up
# rejected inside cosign, where the reason for the rejection is invisible.
# ─────────────────────────────────────────────────────────────────────────────
if [[ -z "$WORKFLOW_REF" && -n "${RELEASE_CERT_WORKFLOW_REF+x}" ]]; then
    echo "::error::RELEASE_CERT_WORKFLOW_REF está definida pero vacía. Un pin vacío no restringe nada y no puede leerse como 'sin pin': quitala del entorno o dale un valor. Un claim que se vacía en silencio convierte este guard en una verificación de forma." >&2
    exit 2
fi

if [[ -z "$WORKFLOW_TRIGGER" && -n "${RELEASE_CERT_WORKFLOW_TRIGGER+x}" ]]; then
    echo "::error::RELEASE_CERT_WORKFLOW_TRIGGER está definida pero vacía. Un pin vacío no restringe nada y no puede leerse como 'sin pin': quitala del entorno o dale un valor." >&2
    exit 2
fi

if [[ -n "$WORKFLOW_REF" && -z "$WORKFLOW_TRIGGER" ]] \
   || [[ -z "$WORKFLOW_REF" && -n "$WORKFLOW_TRIGGER" ]]; then
    echo "::error::--workflow-ref y --workflow-trigger (o RELEASE_CERT_WORKFLOW_REF y RELEASE_CERT_WORKFLOW_TRIGGER) van juntos: pinear uno solo deja el otro claim sin restringir, que es exactamente la forma que este guard existe para cerrar." >&2
    exit 2
fi

if [[ -n "$WORKFLOW_REF" && "$WORKFLOW_REF" != refs/* ]]; then
    echo "::error::--workflow-ref debe ser un ref completo (refs/heads/... o refs/tags/...), no '$WORKFLOW_REF'. El claim del certificado lleva el ref, no el nombre corto." >&2
    exit 2
fi

if [[ -n "$WORKFLOW_REF" && "$IDENTITY_EXPLICIT" != "true" && -z "${RELEASE_CERT_IDENTITY_REGEXP:-}" ]]; then
    echo "::error::--workflow-ref/--workflow-trigger requieren --identity-regexp (o RELEASE_CERT_IDENTITY_REGEXP): la identidad por defecto es tag-only y contradiría los claims pineados, así que la verificación fallaría por el motivo equivocado." >&2
    exit 2
fi

if [[ ${#ASSETS[@]} -eq 0 ]]; then
    echo "::error::no se pasó ningún asset que verificar: una verificación vacía no es una verificación correcta. Lista los assets a comprobar (o usa --help)."
    exit 1
fi

# The value under test, printed. A pin nobody can read is not a pin.
echo "certificate identity regexp: $IDENTITY_REGEXP"
echo "certificate oidc issuer:     $OIDC_ISSUER"
if [[ -n "$WORKFLOW_REF" ]]; then
    echo "pinned workflow ref:      $WORKFLOW_REF"
    echo "pinned workflow trigger:  $WORKFLOW_TRIGGER"
fi

# Fail-closed on a missing verifier. Deliberately BEFORE any per-asset work, so
# the failure cannot be confused with "there was nothing to verify".
if ! command -v "$COSIGN_BIN" >/dev/null 2>&1; then
    echo "::error::cosign no está disponible (probado: '$COSIGN_BIN') y este guard NO se salta: sin verificador, una release sin firma pasaría en verde. En CI lo instala 'sigstore/cosign-installer' (version pineada vía 'cosign-release'); en local, descarga el binario desde https://github.com/sigstore/cosign/releases."
    exit 1
fi

if ! "$COSIGN_BIN" version >/dev/null 2>&1; then
    echo "::error::'$COSIGN_BIN' está en PATH pero no responde a 'version' — no es un cosign utilizable. Bloqueando la release en lugar de omitir la verificación."
    exit 1
fi

VERIFIED=0
FAILED=0

for asset in "${ASSETS[@]}"; do
    if [[ ! -f "$asset" ]]; then
        echo "::error::$asset no existe: no se puede verificar un asset ausente." >&2
        FAILED=$((FAILED + 1))
        continue
    fi

    bundle="${asset}.sigstore.json"
    if [[ ! -f "$bundle" ]]; then
        echo "::error::$asset no tiene bundle '$bundle': el asset está SIN FIRMAR. Un bundle ausente o inválido bloquea la release — no se publica nada sin evidencia de firma." >&2
        FAILED=$((FAILED + 1))
        continue
    fi

    cosign_args=(
        --bundle "$bundle"
        --certificate-identity-regexp "$IDENTITY_REGEXP"
        --certificate-oidc-issuer "$OIDC_ISSUER"
    )
    if [[ -n "$WORKFLOW_REF" ]]; then
        cosign_args+=(
            --certificate-github-workflow-ref "$WORKFLOW_REF"
            --certificate-github-workflow-trigger "$WORKFLOW_TRIGGER"
        )
    fi

    if "$COSIGN_BIN" verify-blob "${cosign_args[@]}" "$asset"; then
        echo "OK signature verified: $asset"
        VERIFIED=$((VERIFIED + 1))
    else
        echo "::error::firma inválida, ausente en el log de transparencia, o de una identidad ajena: $asset. No se publica." >&2
        FAILED=$((FAILED + 1))
    fi
done

TOTAL=${#ASSETS[@]}
echo "release signature verification: $VERIFIED/$TOTAL verified, $FAILED failed"

if [[ "$FAILED" -ne 0 ]]; then
    echo "::error::$FAILED de $TOTAL assets no superaron la verificación de firma. La release NO se crea: publicar un asset sin firma verificable sería exactamente el defecto que este guard existe para cerrar."
    exit 1
fi

if [[ "$VERIFIED" -ne "$TOTAL" ]]; then
    # Unreachable given the accounting above, and kept anyway: a counter that
    # does not reconcile must not read as a pass.
    echo "::error::la contabilidad no cuadra ($VERIFIED verificados de $TOTAL assets) — fallando en vez de asumir éxito."
    exit 1
fi
