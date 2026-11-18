# Closure: ai-egress-proxy — ADR acceptance, profile `.no_proxy()`, CONNECT evidence

Branch: `fix/ai-egress-closure` (from `origin/main` `aba0e9cf`). Delivery: one PR, `Closes #2551`.
Test runner: `cargo nextest`. Local gate: `bash scripts/ci_fast_gate.sh` (full, not only focused).

## Specs

- S1 (owner, 2026-11-18): "` .no_proxy() ` ausente en `secure_client_with_profile` (infrastructure/ssrf.rs). El ADR, Decisión 2, exige `.no_proxy()` incondicional antes de instalar el matcher; la implementación lo comenta pero no lo llama." Fix the call and pin the profile-direct arm with a test that fails when ambient proxy env is honored.
- S2 (owner, 2026-11-18): "Sin cobertura de HTTPS CONNECT/TunnelConnector. El harness con wiremock solo prueba absolute-form para `http://` — el ADR ... pedía que el túnel se afirmara a nivel builder/wiring o con un fake con capacidades de túnel." Add that assertion; no production change.
- S3 (owner, 2026-11-18): "El ADR sigue marcado como Proposed — draft for owner review. ... el documento debería decir Accepted (o similar)."
- S4 (owner, 2026-11-18): "Issue #2551 sigue OPEN con los cinco acceptance hints del body todavía en - [ ]." Check every hint against merged evidence and close the issue after merge.
- S5 (owner, 2026-11-18): "corrí los tests focalizados, no el gate completo (ci_fast_gate.sh) hoy". Run the full local gate in this closure and report its observed result.

## Tasks

- T1 (S1): add `.no_proxy()` between the redirect policy and the proxy matcher in `DefaultSsrfGuard::secure_client_with_profile`; add a one-test integration target that sets ambient proxy env, builds a direct profile client through the guard, and proves the destination (not the ambient proxy) receives the request. Route: inline. Risk: item 2 (security guard). Status: done — `tests/ai_proxy_profile_direct_test.rs`; observed RED with `.no_proxy()` removed (`destination received 0, right 1`), GREEN restored.
- T2 (S2): add a builder/wiring-level HTTPS tunnel assertion: guard-built profile client through a loopback proxy, raw TCP fake accepts the `CONNECT host:443` line before TLS fails. Route: inline. Risk: item 6 only if the assertion cannot fail; design it to fail on a missing/incorrect tunnel. Status: done — `tests/ai_proxy_connect_test.rs`; mutation removing `.proxy(...)` failed with `the client must dial the configured proxy: Elapsed(())`, restored GREEN.
- T3 (S3): update ADR-0021 status to Accepted with the merge evidence. Route: inline. Risk: none (passive doc). Status: done — status is `Accepted — 2026-11-18`; Slice 4 names the CONNECT test.
- T4 (S4): verify all five #2551 acceptance hints against the merged slices, check them in the issue body, and close the issue after this PR merges. Route: inline. Risk: none (external bookkeeping). Status: partial — all five body hints are now `- [x]`; the issue stays OPEN until the merge closes it.
- T5 (S5): run focused proxy targets, full `ci_fast_gate.sh`, and the pre-commit gate; record observed output. Route: inline. Risk: none. Status: done — 40/40 focused PASS; full gate `PASS=45 FAIL=0 SKIP=2`.

## Log

- L1 (2026-11-18, verbatim owner follow-up): "No, no al 100%. La implementación funcional sí está mergeada, pero quedan cuatro cabos sueltos. ... Los próximos pasos obvios serían, en este orden: arreglar el `.no_proxy()` (es una línea, con su test), cerrar el punto de CONNECT con una aserción de wiring, actualizar el Status del ADR, y marcar/cerrar #2551."
- L2: verified on `origin/main` `aba0e9cf`: `secure_client_with_profile` comments `.no_proxy()` but does not call it; ADR-0021 line 3 still says Proposed; issue #2551 still OPEN with five unchecked hints; no CONNECT assertion found in the proxy tests.
- L3 (2026-11-18, T1/T2 evidence): T1 edit is `.redirect(...).no_proxy()` in `DefaultSsrfGuard::secure_client_with_profile`; `ai_proxy_profile_direct_test::profile_direct_arm_ignores_ambient_proxy_env` was RED only with the call removed and is GREEN with it. T2's raw loopback fake asserts `CONNECT ai.invalid:443 HTTP/1.1`; removing the `.proxy(...)` arm makes the test fail with `the client must dial the configured proxy: Elapsed(())`.
- L4 (2026-11-18, T5 evidence): focused proxy suite 40/40 PASS (`ai_proxy_profile_direct_test`, `ai_proxy_connect_test`, `ai_proxy_e2e_test`, `ai_proxy_407_test`, `proxy_posture_e2e_test`). First full gate run failed 4 steps: golden real-tree sites (line shift from `.no_proxy()`), test-inventory drift (two new test files), clippy (`tokio::spawn` in the new test), and a rustdoc ICE. Repairs: refreshed `tools/intra-crate-lint/tests/fixtures/real_tree_sites.txt`, regenerated `docs/test-inventory.json`, added the test-file clippy allow, ran `cargo fmt --all`; final full gate with `PATH=$HOME/.cargo/bin:$PATH` (bypassing the local mbx cargo shim that ICEs rustdoc) reports `PASS=45 FAIL=0 SKIP=2` and `fast-gate: GREEN`.
- L5 (2026-11-18, T4 state): issue #2551 body now has all five acceptance hints as `- [x]`; state remains OPEN. Merge of the closure PR with `Closes #2551` closes it.
