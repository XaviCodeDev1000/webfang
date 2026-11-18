# Architecture Decision Records — index

This directory is the project's body of architectural decisions. This index is the
**citable** surface: an ADR number without this map is ambiguous, and a decision that
cannot be cited cannot be audited.

## Conventions

| Convention | Rule |
| :--- | :--- |
| File name | `NNNN-slug.md`, four digits, kebab-case slug |
| Status | `**Status:**` in the file header — `Accepted`, `Proposed`, `Superseded by NNNN`, `Deprecated` |
| Citation in code | `ADR-NNNN` (or the key in the table below when the number is shared) |
| Numbering | A number is never reused. A gap is history, not an error |

## Index

| Citation key | File | Decision | Status |
| :--- | :--- | :--- | :--- |
| `ADR-0001` | [0001-unify-asset-downloader.md](0001-unify-asset-downloader.md) | Unify asset downloading in the adapters `Downloader` | Accepted (amend. 2026-09-04) |
| `ADR-0003` | [0003-remote-inference-adapter.md](0003-remote-inference-adapter.md) | OpenAI-compatible remote inference adapter as the default AI path | Accepted |
| `ADR-0004` | [0004-onnx-local-removal.md](0004-onnx-local-removal.md) | Retire local ONNX; remote embeddings as the only path | Accepted — shipped |
| `ADR-0009` | [0009-persistence-mode-input-struct.md](0009-persistence-mode-input-struct.md) | `PersistenceMode` input struct: the domain owns the rule, application the wiring | Accepted |
| `ADR-0010` | [0010-intra-crate-direction-allowlist.md](0010-intra-crate-direction-allowlist.md) | Strict intra-crate direction gate: hybrid ports + allowlist | Accepted |
| `ADR-0011` | [0011-tighten-intra-crate-allowlist.md](0011-tighten-intra-crate-allowlist.md) | Tighten the allowlist: remove broad `infrastructure::` | Accepted |
| `ADR-0012` | [0012-intra-crate-allowlist-roadmap.md](0012-intra-crate-allowlist-roadmap.md) | Roadmap to empty the intra-crate allowlist (#994) | Accepted |
| `ADR-0012-B` | [0012-intra-crate-allowlist-roadmap-b.md](0012-intra-crate-allowlist-roadmap-b.md) | Corrected 10→2 allowlist plan | **Proposed** — *stale*: the plan has been complete since 2026-09-03 and the allowlist is at 2 entries |
| `ADR-0012-B-RUNBOOK` | [0012-B-RUNBOOK.md](0012-B-RUNBOOK.md) | Runbook of the measured operational state of 0012-B (not a decision) | n/a (runbook) |
| `ADR-0013` | [0013-state-store-port-seam.md](0013-state-store-port-seam.md) | `StateStorePort`: domain seam over the legacy state store | Accepted |
| `ADR-0014` | [0014-persistence-unification-hybrid-d3.md](0014-persistence-unification-hybrid-d3.md) | Persistence unification and hybrid D3 commit-point strategy | Accepted |
| `ADR-0015` | [0015-waf-detection-stays-infrastructure-side.md](0015-waf-detection-stays-infrastructure-side.md) | WAF detection stays on the infrastructure side (#994 slice 4) | Accepted |
| `ADR-0016` | [0016-interruption-resume-lifecycle.md](0016-interruption-resume-lifecycle.md) | Interruption/resume lifecycle: single owner, precedence fixed, residual F-R3-7 | Accepted |
| `ADR-0016-POST-LOAD-WAIT` | [0016-post-load-wait-policy.md](0016-post-load-wait-policy.md) | Post-load wait policy: network-idle by default (F-52-b) | Accepted — shipped |
| `ADR-0017` | [0017-crawlsession-run-owner.md](0017-crawlsession-run-owner.md) | `CrawlSession`: a single owner per run | Accepted |
| `ADR-0018` | [0018-egress-strategy-without-third-party-proxies.md](0018-egress-strategy-without-third-party-proxies.md) | Egress strategy without third-party proxies | Proposed — draft; narrowed for AI egress by `ADR-0021` |
| `ADR-0019` | [0019-boundedness-lives-in-the-capture-sink.md](0019-boundedness-lives-in-the-capture-sink.md) | Boundedness lives in the capture sink; blocking is out of scope | Accepted |
| `ADR-0020` | [0020-shared-llm-transport-primitives.md](0020-shared-llm-transport-primitives.md) | Shared transport primitives with per-port policies for OpenAI-compatible clients | Accepted |
| `ADR-0021` | [0021-explicit-ai-egress-proxy.md](0021-explicit-ai-egress-proxy.md) | Explicit per-provider proxy for AI egress with SSRF policies separated by type and no environment reading | Proposed — draft (decision of `#2551`) |

**Total: 19 files, 18 decisions + 1 runbook.**

## Numbering gaps

`0002`, `0005`, `0006`, `0007` and `0008` **never existed**. Verified with
`git log --diff-filter=A -- "docs/adr/*"`: no file with those numbers was ever added.
There is no history loss to look for.

## Why there are shared numbers

`ADR-0012` has three files and `ADR-0016` two. It is not a typo: they are decisions
written in the same wave, and the runbook inherited the number of the plan it documents.

**These files are NOT renumbered**, and the reason is measurable: `ADR-0012` appears **143
times** in code, scripts and documentation; renaming the file without migrating the
citations breaks more than it fixes. The ambiguity is resolved **in this index**, which is
the surface a reader consults:

- `ADR-0012` → the roadmap (accepted), the original plan.
- `ADR-0012-B` → the corrected plan (10→2), the one that was executed.
- `ADR-0012-B-RUNBOOK` → the measured operational state, not a decision.

The same with 0016:

- `ADR-0016` → interruption/resume lifecycle. **It is the one cited with `§4`, `§5` and
  `§6`** (P8-4/P8-5/P8-6, residual F-R3-7).
- `ADR-0016-POST-LOAD-WAIT` → post-load wait policy (F-52-b). It has no numbered sections
  cited from code.

A real renumbering (with migration of the 143 citations) is open debt, not a change to
this index.

## Rules for a new ADR

1. Take the next free number **after the maximum**, never a gap.
2. Write `**Status:**` in the header, with the date if the status changed.
3. Add the row to this index's table **in the same commit**. An ADR without a row cannot
   be cited.
4. If the decision replaces another, put `Superseded by NNNN` on the old one and
   `Supersedes` on the new one.