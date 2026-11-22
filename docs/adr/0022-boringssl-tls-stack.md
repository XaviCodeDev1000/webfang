# ADR 0022: BoringSSL via wreq stays the TLS stack — CMake is an upstream build requirement

- **Status:** Accepted
- **Date:** 2026-11-22
- **Deciders:** Project maintainer (repository policy owner)
- **Related:** ADR-0003 §3, issues #1952, #1797, #2610, `odd/tasks/build-perf-experiments.md`

## Decision

Keep `wreq` → `btls` → `btls-sys` → BoringSSL as the only TLS stack. Do not replace the `cmake` build-dependency with `cc`, do not switch to a system or prebuilt BoringSSL, do not migrate to `rustls`/`ring`/`aws-lc`, and do not use `reqwest`. Treat CMake as an **upstream toolchain prerequisite**, not a Webfang build decision. Any future change here is a product decision about TLS fingerprint impersonation and must supersede this ADR and ADR-0003 §3 explicitly.

## Context

### The repo owns almost none of this build

Webfang has no `CMakeLists.txt`. Its only build script, `crates/webfang_core/build.rs`, is 18 lines: one `cargo:rerun-if-changed` directive, the `cfg(loom)` gate from ADR-0014, and a comment recording the `built`/`git2`/`libgit2-sys` purge (issue #1797).

The external requirement enters through one verified edge in `Cargo.lock`: `wreq 6.0.0-rc.29` → `btls 0.5.6` → `btls-sys 0.5.6`, whose build-dependencies are `bindgen`, `cmake`, `fs_extra` and `fslock`. `cmake 0.1.58` depends on `cc 1.3.0`, and `btls-sys` is the only crate in the lock that depends on `cmake`. `NOTICE` §4 ("OpenSSL and BoringSSL — statically linked") records that `btls-sys` compiles BoringSSL from C++ and links it statically. No crate named `rustls`, `ring` or `aws-lc` is in the lock; `rustls-pki-types` is a type crate pulled by `webpki-root-certs`, not a TLS stack.

### Impersonation is load-bearing, not decorative

ADR-0003 §3 states: "HTTP client is `wreq`, never `reqwest`. Non-negotiable repository policy: TLS fingerprint impersonation and the existing `ValidatingResolver` integration come with `wreq`." `AGENTS.md` restates it: "HTTP: always `wreq`, never `reqwest` — TLS fingerprint impersonation for WAF evasion."

This is wired, not asserted. `build_wreq_client` in `crates/webfang_core/src/application/http_client/factory.rs` applies `.emulation(config.tls_emulation)` and Chrome 145 Client Hints under the comment "These MUST match the TLS fingerprint to avoid \"Headless Spoofing\" detection", and installs `crate::domain::ssrf_guard::ssrf_guard().secure_client(builder)` at the same point.

Field evidence in `odd/tasks/jitter-pacing-2610.md` (issue #2610): "con delay fijo de 1,5 s y 4 s, el sitio tropezó repetidamente por tasa mientras que las peticiones aisladas (fingerprint TLS de wreq) siempre pasaron con 200. El fingerprint ya no es el cuello de botella; el patrón temporal sí aporta señal." The fingerprint was sufficient; the temporal pattern was the problem. That is evidence about what to optimize, not a licence to remove the fingerprint.

### The build cost is measured and off the critical path

`odd/tasks/build-perf-experiments.md`, finding #3: "The C build scripts are NOT the critical path, so fixing C build time does not help." At opt-level 3, `btls-sys` was 70.8 s and the then-present `libgit2-sys` 113.1 s; at `-O0` the C total moved only 4.4 s, while ~1958 s of rustc work ran concurrently on 16 cores. T3 (`CFLAGS/CXXFLAGS=-O0`) was 190.0 s against 194.3 s baseline — 2%, noise. `libgit2-sys` has since been purged (issue #1797), so only `btls-sys` remains.

A controlled re-measurement on 2026-11-22 (cold target, real cargo with the mbx shim bypassed, `cargo build -p webfang_core --features chromium --timings`, rustc 1.91.1, 16 jobs) gave a 132.3 s build of 540 units in which the `btls-sys` build script took 74.2 s. It remains one concurrent unit, not the wall.

### The real price is portability, not seconds

- `.github/workflows/release.yml` must set `CC_aarch64_unknown_linux_gnu` / `CXX_aarch64_unknown_linux_gnu`: `btls-sys` configures CMake's compilers from those variables only, or the cross link dies with "Relocations in generic ELF (EM: 62)".
- `docs/src/installation.md`: the binary links `libstdc++` and `libgcc_s` because BoringSSL is C++; there is no musl or fully static artifact.
- `cfg-windows-lint` in `.github/workflows/ci.yml` documents "CMake Error at CMakeLists.txt:374 (enable_language): No CMAKE_ASM_NASM_COMPILER could be found." — resolved with a C++ cross compiler plus `nasm`.
- `fuzz.yml`, `sanitizers.yml` and `mutants.yml` install `cmake` explicitly; Miri must ignore the FFI-hostile tests that call into `btls-sys`.

## Consequences

- `cmake`, a C/C++ toolchain and (on Windows cross-lint) NASM stay hard prerequisites; cross-compilation keeps its target-specific compiler variables; the runtime keeps its glibc/libstdc++ dependency and Miri keeps its FFI carve-out.
- Build-performance work stays on measured levers: the landed T5 dev profile, local-only mold, and mbx for cross-target-dir hits. `AGENTS.md` already records these. Chasing the C build time is explicitly not one of them.
- Chromium needs no further gating: `chromium = ["dep:chromiumoxide"]` is optional with `default = []`, `scripts/test_build_graph.sh` pins the browser-free default graph, and only explicitly all-features/nightly lanes pay for CDP. The earlier 47.4 s figure was a misattributed measurement; the re-measured 75.0 s for `chromiumoxide_cdp v0.7.0` belongs to a `--features chromium` build and is not comparable to default-feature runs.
- Follow-up outside this decision: `NOTICE` also claims `ring 0.17.14` and `rustls-webpki` are statically linked, but neither appears in `Cargo.lock`. That is a documentation-accuracy issue to verify and correct separately, not a change to this stack.

## Alternatives rejected

- **Replace the `cmake` crate with `cc`.** A category error: `cmake` is already a native Rust build-dependency that shells out, and `cc` is already in the graph through it. The non-Rust part is BoringSSL's own CMake build system and its hand-written NASM assembly; swapping the wrapper removes neither.
- **System or prebuilt BoringSSL.** `btls-sys` exposes no such option in this repository. It is an unverified upstream hypothesis, not a fact to build on.
- **`rustls`, `ring` or `aws-lc`.** Removes TLS fingerprint impersonation. The field evidence shows impersonation is not the current bottleneck; removing it is a product decision, not a dependency swap.
- **`reqwest`.** Forbidden by ADR-0003 §3 and `AGENTS.md`.
- **Optimizing the C build scripts.** Measured as off the critical path (finding #3, T3 at −2%); the largest C consumer, `libgit2-sys`, was already removed.
