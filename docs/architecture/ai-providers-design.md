# AI Provider — Final Design

> **Status**: Final design for the remote AI providers feature (embeddings + LLM completions).
> Implementation in `crates/webfang_core/src/infrastructure/llm/` (`client.rs`, `provider.rs`, `remote_embedding.rs`, `mod.rs`).
>
> **ADR-0004 step 6 update:** local ONNX was removed (the `webfang_ai` crate deleted, the `ai` feature retired). The local ONNX `EmbeddingAdapter` no longer exists; the only living embeddings adapter is the remote one (`RemoteEmbeddingAdapter`/`EmbeddingPort` in `webfang_core`), and the `ProviderKind::LocalOnnx` variant proposed below lost its basis (`InferencePool` no longer exists) — if the provider registry lands, it arrives without the local variant or with a new ADR.

---

## Current state (what exists today)

### Domain traits (already existing)

- **`EmbeddingPort`** (`domain/embedding_port.rs`) — open trait, not sealed, for embedding vector generation:
  - `embed(text) -> Vec<f32>` — one embedding per text
  - `embed_batch(texts) -> Vec<Vec<f32>>` — batched (overridable)
  - `embedding_dim() -> usize` — declared dimension
  - `model_tag() -> &str` — model tag (for namespacing in the vault)
  - Existing implementations: `RemoteEmbeddingAdapter` (remote HTTP, in `webfang_core` infra). The local ONNX `EmbeddingAdapter` was removed in ADR-0004 step 6

- **`LlmPort`** (`domain/llm_port.rs`) — open trait, not sealed, for LLM completions:
  - `send_completion(request: LlmRequest) -> LlmResponse` — a single completion
  - Existing implementation: `OpenAiLlmClient` (OpenAI-compatible, with `base_url` configurable as a construction parameter)

### Existing infrastructure

- **`OpenAiLlmClient`** — OpenAI-compatible LLM client; **it is not a generic provider**, it is a concrete client that can point at any OpenAI-compatible endpoint (OpenRouter, FreeLLM, etc.) by configuring `base_url`. It does not support multi-tenant (one client = one tenant/key).
- **`RemoteEmbeddingAdapter`** — HTTP embeddings adapter for remote providers (the one formerly known as "this feature").
- **`build_default_http_client()`** — function that builds the `wreq::Client` with canonical config (Chrome145, 60s timeout, 10s connect, SSRF guard armed). **Extracted from the `OpenAiLlmClient` constructor** to be reusable.
- **`OpenAiLlmClient::with_http(http, base_url, api_key)`** — alternative constructor that accepts an already-built `wreq::Client` (to share the pool).
- **`CredentialStore`** — in-memory credential store with `ApiKey`/`AccessToken` (wrappers of `SecretString` with zeroize and redacted Debug). **Does not persist to disk**.

### Limitations of the current design

1. **No per-provider credential management:** remote providers' API keys are injected via env vars or direct construction; there is no way to say "this key is for OpenRouter, this other one for FreeLLM".
2. **No multi-tenant support:** `OpenAiLlmClient` (and `RemoteEmbeddingAdapter`) are singletons per instance; if you need two tenants with different keys/base_urls, you need two separate instances and there is no registry to manage them.
3. **No secure secret persistence:** `CredentialStore` is memory-only; there is no encrypted persistent storage mechanism.
4. **No notion of "provider with capabilities":** there is no way to declare "this provider does embeddings, this other one does completions, this other one does both".

---

## New concepts

### 1. `AuthSource` — source of a credential

Enum describing where a provider's API key comes from:

```rust
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "source", rename_all = "snake_case")]
pub enum AuthSource {
    /// Credential in the system keyring (libsecret / macOS keychain / Windows Credential Manager)
    Keyring { service: String, account: String },
    /// Credential in an age-encrypted file on disk
    EncryptedFile { path: PathBuf },
    /// Credential in an environment variable (legacy, CI or development only)
    Env { var: String },
    /// No credential: the endpoint is anonymous by design (B1).
    ///
    /// Exists so that a local endpoint without a key (Ollama, LM Studio, an
    /// embeddings server of your own) is expressible.
    None,
}
```

Implemented in `crates/webfang_core/src/domain/auth_source.rs:58`; the `None` variant
is at `:102`.

**Design decisions:**

- **`None` is a declared source, not an absence.** The `auth` field is still
  mandatory in the config file: `{"source": "none"}` is an explicit statement,
  and its absence remains a parse error. Before this variant, `resolve()` had no
  "no credential" return, and an anonymous endpoint was inexpressible by
  construction.
- **No `Inline` in v1** — writing the key literally in a config file (even an age-encrypted TOML) is a security risk we have no real mitigation for (a log warning is absolution, not mitigation; someone in a hurry will ignore it). If it is justified in the future, it will arrive with `SecretString` and its own discussion.
- **No implicit fallback** — a failing `AuthSource::Keyring` does NOT try `EncryptedFile` or `Env`. The user must explicitly configure which source to use, and if that source fails, it fails explicitly with that source's error. This is intentional: a silent `Keyring → EncryptedFile → Env` fallback would mean a user believes they are using the keyring and ends up on an env var without knowing, inheriting a vulnerability.
- **No `Default` implemented** — `AuthSource` does not implement `Default`: the config file requires an explicit `auth` block. What exists is a setup *recommendation* (`DetectedStore::recommendation`, `crates/webfang_core/src/domain/local_secret_store.rs:37`): `EncryptedFile` (age) on Linux always —the kernel keyring is memory-only and is lost on reboot— and `Keyring` on macOS/Windows when the store is available. The environment variable is never the recommendation, precisely so as not to install the habit of "putting the key in an environment variable" as a canonical practice.

### 2. `AuthError` — credential resolution errors

```rust
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    #[error("keyring not available on this system")]
    KeyringUnavailable,
    #[error("no credential found for {service}/{account}")]
    CredentialNotFound { service: String, account: String },
    #[error("keyring error: {0}")]
    KeyringFailure(String),
    #[error("credential file not found: {0}")]
    FileNotFound(PathBuf),
    #[error("could not read the credential file {0}: {1}")]
    FileRead(PathBuf, String),
    #[error("could not decrypt the credential file {0}: {1}")]
    Decrypt(PathBuf, String),
    #[error("invalid credential format in {0}")]
    FileFormat(PathBuf),
    #[error("environment variable {0} not set")]
    EnvNotSet(String),
    #[error("invalid credential: {0}")]
    Invalid(String),
}
```

Implemented in `crates/webfang_core/src/domain/auth_source.rs`: the four failures
`FileNotFound`/`FileRead`/`Decrypt`/`FileFormat` come from the same encrypted file, but they
are distinct errors and are reported separately: "does not exist" is not "could not be read"
nor "could not be decrypted".

### 3. `AuthSource::resolve()` — obtain the credential

```rust
impl AuthSource {
    pub fn resolve(&self) -> Result<Option<ApiKey>, AuthError> {
        match self {
            Self::None => Ok(None),
            Self::Keyring { service, account } => resolve_keyring(service, account).map(Some),
            Self::EncryptedFile { path } => resolve_encrypted_file(path).map(Some),
            Self::Env { var } => resolve_env(var).map(Some),
        }
    }
}
```

Real signature: `pub fn resolve(&self) -> Result<Option<ApiKey>, AuthError>`
(`crates/webfang_core/src/domain/auth_source.rs:118`). The `Option` in the return is
load-bearing: `Ok(None)` means **"this endpoint is anonymous"** and only the
`None` variant produces it; `Err` means there was a configured credential and it could not
be resolved. An empty `Authorization` header is never synthesized —the absence of a
credential is expressed as an absence of header.

Each variant handles its own resolution logic and returns a specific `AuthError` on failure. There is no orchestrator that tries the next source.

### 4. `ProvidersConfig` — provider configuration

```rust
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ProvidersConfig {
    pub providers: Vec<ProviderConfig>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct ProviderConfig {
    pub id: String,
    pub display_name: String,          // human-readable provider name
    pub kind: ProviderKind,
    pub base_url: Url,
    pub auth: AuthSource,
    pub capabilities: Vec<Capability>,
    pub model: Option<String>,         // default model for this provider
    #[serde(default, deserialize_with = "deserialize_dim_no_null")]
    pub embedding_dim: Option<usize>,  // explicit dimension (if it differs from the default model)
    #[serde(default)]
    pub allow_loopback: bool,          // enables dials to loopback (127.0.0.1 / ::1)
    #[serde(default)]
    pub network_policy: NetworkPolicy, // allowlist by CIDR/host (mode: "allowlist")
    #[serde(default, deserialize_with = "deserialize_proxy_spec_no_null")]
    pub proxy: Option<ProxySpec>,
    #[serde(default, deserialize_with = "deserialize_extra_body")]
    pub extra_body: ExtraBody,
    #[serde(default, deserialize_with = "deserialize_extra_body")]
    pub query_extra_body: ExtraBody,
    #[serde(default = "default_send_encoding_format")]
    pub send_encoding_format: bool,
}
```

Implemented in `crates/webfang_core/src/domain/providers.rs:152`. The `#[serde(default)]` are
not decorative: `allow_loopback` and `network_policy` are optional in the file, so absent
means `false` and the default policy (`NetworkPolicyMode::Restricted`: only public
endpoints, with loopback governed solely by `allow_loopback`), and an explicit `null` is a
deserialization error, never a silent "adopt the default". `embedding_dim` also carries
`deserialize_with = "deserialize_dim_no_null"` for the same reason. `allow_loopback` **has**
no CLI flag on purpose —flags are filtered into the process list and would relax an
SSRF protection—: the decision travels per client, never as registry state.

#### Vendor-specific request fields (#3074)

`extra_body` is flattened into the root of `/embeddings` and `/chat/completions`
requests; WebFang writes its own fields last, so an operator map can never replace
`model`, `input`, `messages` or the mapped completion parameters. `query_extra_body`
is an overlay applied by vault search: base keys stay, overlay keys win, so an
asymmetric model indexes with `passage` and queries with `query`.

`RESERVED_REQUEST_KEYS` (`crates/webfang_core/src/domain/providers.rs:98`) are
rejected at deserialize time: `model`, `input`, `messages`, `temperature`, `auth`,
`response_format`, `max_tokens`, `max_completion_tokens` and `encoding_format`.
Reserved keys are protocol-critical at the body root; a nested object may reuse
one legitimately, so the check is top level only.

`send_encoding_format` defaults to `false` via `default_send_encoding_format()`.
This resolves the AC4/AC6 conflict in #3074: the pre-issue `/embeddings` body never
carried `encoding_format`, and Voyage/Gemini reject the field while NVIDIA NIM already
defaults to float. Keeping the default byte-identical preserves every existing
provider; opting in sends `encoding_format: "float"` only for gateways that default
to base64.

Where:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    OpenAiCompatible,   // OpenAI-compatible API (OpenAI, OpenRouter, FreeLLM, Groq, Together, Ollama)
    // NOTE ADR-0004 step 6: the LocalOnnx variant proposed here was left without a basis
    // (there is no `InferencePool` left to reuse); it was removed from the code in the
    // same step that deleted the `webfang_ai` crate.
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Embedding,
    Completion,
}
```

The `rename_all = "snake_case"` is not cosmetic: it is what produces the keys
`open_ai_compatible` and `embedding` in the config file. Without it, the same
code serializes with a different casing and the config stops parsing.

### 5. `ProviderRegistry` — provider registry by ID

```rust
pub struct ProviderRegistry {
    config: ProvidersConfig,
}
```

It is not a trait, it is a concrete struct with lookup logic by ID.

**Resolution by capability:** instead of `AnyProviderHandle` (removed in round 3 as redundant against the slot-based design), the registry resolves directly by capability, with a single lookup method:

```rust
impl ProviderRegistry {
    pub fn resolve(
        &self,
        provider_id: &str,
        capability: Capability,
    ) -> Result<&ProviderConfig, RegistryError>

    pub fn resolve_default_completion(&self) -> Result<&ProviderConfig, RegistryError>

    pub fn resolve_default_embedding(&self) -> Result<&ProviderConfig, RegistryError>
}
```

Verified in `crates/webfang_core/src/domain/providers.rs:323` (companions at `:377` and `:395`), with `RegistryError::ProviderNotFound` at `:67` and `RegistryError::CapabilityMismatch` at `:73`.

**The registry returns a reference to the config, not a port.** The adapter construction —and therefore the `Arc<dyn EmbeddingPort>` / `Arc<dyn LlmPort>`— happens in the binary layer, not here: `build_llm_provider` / `build_embedding_provider` in `crates/webfang_core/src/cli/llm_wire.rs`. That separation is deliberate: the domain does not know which concrete adapter implements each `ProviderKind`.

Capabilities do not need their own method: they are read from `ProviderConfig.capabilities` (`Vec<Capability>`, `providers.rs:165`), and `resolve()` validates against that list before returning the config. The design does not include a separate capability accessor.

This allows a caller to resolve a provider by slot (capability) without going through a generic handle. The real consumers of the registry are the embeddings config resolution (`crates/webfang_core/src/cli/llm_wire.rs:74` and `:80`) and the `webfang doctor` diagnosis (`crates/webfang_core/src/cli/commands/doctor.rs:63` and `:78`). `VaultSearchService` **is not** one of them: it receives its `Arc<dyn EmbeddingPort>` by injection at construction (`crates/webfang_core/src/application/vault_search.rs:70`) and never consults the registry — citing it as a registry consumer would be false.

### 6. `TenantAiContext` — multi-tenant context (retired contract)

> **Retired.** `TenantAiContext` **was never built**: zero occurrences in `crates/`. The contract is retired as a promised API surface —owner decision recorded in issue **#2018**—: the design intent (one context per tenant that resolves the provider already chosen by slot) is kept there and reopened when a second real tenant exists, not as a type promised in this document.

The `wreq::Client` scalability note that accompanied this contract lives in the "On shared `wreq::Client`" section of the Architecture notes.

### 7. `DetectedStore` — setup helper, not a runtime resolver

> **Renamed in the implementation.** This design called the type `LocalSecretStore`; what was implemented is `DetectedStore`, in `crates/webfang_core/src/domain/local_secret_store.rs:16` (`detect()` at `:48`). `LocalSecretStore` remained only as a module name.

`DetectedStore::detect()` is a setup helper that suggests to the user which credential sources are available on their system during initial configuration:

```rust
pub struct DetectedStore {
    pub keyring_available: bool,
    pub encrypted_file_path: Option<PathBuf>,
    pub env_vars_detected: Vec<String>,
    pub recommendation: AuthSource, // recommended source for storing the next credential
}
```

`recommendation` (`crates/webfang_core/src/domain/local_secret_store.rs:37`) is the source
the setup suggests to the user: `EncryptedFile` (age) on Linux always —the kernel keyring
is memory-only and is lost on reboot— and `Keyring` on macOS/Windows when the system
store is available.

**It is not a runtime resolver.** During normal execution, `AuthSource::resolve()` is the one that obtains the credentials. `DetectedStore::detect()` is only used in the initial setup wizard to guide the user.

### 8. Wizard — UX contract (pending implementation, decisions closed)

The product decisions of the wizard, fixed in review before writing it:

- **(a) First run on Linux: generate the identity automatically** (SSH model:
  file with 0600, no interactive passphrase). But the wizard **says it out
  loud**: "I am going to create an identity in `~/.config/webfang/identity.key`;
  if you lose it, you will have to rotate your providers". Without that message,
  `EncryptedFile` looks "secure by magic" and the user never makes a backup.
- **(b) Permissive Container, binary validates at startup.** The Container does NOT fail
  at construction without a provider; but every service binary that starts with
  an LLM feature enabled MUST validate `llm_port().is_some()` at startup and
  exit with a clear error if it is missing (it fails at startup if the feature is
  enabled; it does not fail if it is not). Without that validation the failure arrives late
  on the first call — a daemon that says OK at startup and fails when a
  user/cron invokes the feature, with retries amplifying before
  anyone notices. `absent_llm_port_is_config_error` tests the Container's
  behavior, it does not justify it as a service contract.
  **Expiry of the MUST**: the lint/test that enforces it is born with the first
  binary that exposes a flag like `--extract-with-llm` — that PR does not merge
  without it. A MUST without a date is the eighth binary without validation that nobody noticed.
- **(c) Two flows, no unifying abstraction.** Linux (`EncryptedFile`:
  identity + permissions + rotation) and macOS/Windows (`Keyring` if the backend
  responds) are different flows. The wizard implements them separately; a
  common abstraction that "simplifies" the Linux case is a design bug.
- **Rotate, not "re-configure".** If `identity.key` is lost, ALL
  ciphertexts encrypted with it become undecipherable at once (as many
  providers as use that identity). The wizard offers an explicit
  **rotate** command: generates a new identity, re-encrypts what the user provides
  again, deletes the orphaned `.age` files. "Re-configure" leaves dead files
  lying around that look in use.

---

## Proposed implementation flow

### Phase 0: Support infrastructure (no visible features)

1. **`AuthSource` + `AuthError` + `resolve()`** with unit tests per variant (keyring, encrypted_file, env), including error cases.
2. **`DetectedStore::detect()`** → informative diagnosis (`domain/local_secret_store.rs:48`).
3. **`build_default_http_client()`** extracted and reusable.
4. **`OpenAiLlmClient::with_http(http, base_url, api_key)`** — additive, `new()` intact.
5. **`ProvidersConfig`** parsing + `ProviderRegistry` with resolution by ID and by capability.

### Phase 1: Concrete OpenAI-compatible provider

6. **`OpenAiCompatibleProvider`** — struct implementing `LlmPort` (and potentially a future `EmbeddingPort`) using `OpenAiLlmClient` internally:
   - Builds the client with `with_http(build_default_http_client()?, base_url, api_key_resolved)`.
   - `api_key_resolved` comes from `AuthSource::resolve()` configured in `ProviderConfig`.
7. **Contract fixtures** (`tests/fixtures/{openai,openrouter,freellm,groq}.json`) against the provider's response parser.

### Phase 2: Container integration

8.  **`Container::with_providers_config(config: ProvidersConfig)`** — injects the registry.
9. **`Container::resolve_provider(provider_id)`** — resolution by ID for existing callers.
10. Integration tests: container + registry + concrete provider.

---

## Acceptance criteria

### Security criteria

- `[x]` `AuthSource::resolve()` does not do implicit fallback between sources — a single-source `match` per variant (`domain/auth_source.rs:118-125`), no orchestrator.
- `[x]` `AuthSource::Keyring` fails with `AuthError::KeyringUnavailable` if the keyring is unavailable, does not try another source (`resolve_keyring`, `auth_source.rs:151`).
- `[x]` `AuthSource::Env` is supported but documented as "legacy/CI-only", not as the default (`auth_source.rs:72`: "Credential in an environment variable (legacy, CI or development only)").
- `[x]` `Inline` is not available in v1 (absent from the enum) — explicitly declared in `auth_source.rs:6`.
- `[ ]` The API key never appears in logs, traces, or error messages (redacted `Debug` in `ApiKey`, `SecretString`).
- `[x]` `DetectedStore::detect()` is not used as a runtime credential resolver (`domain/local_secret_store.rs:48`; the one that resolves is `AuthSource::resolve()`).
- `[ ]` Credentials persisted to disk (if `EncryptedFile` is implemented in v1) use age encryption, not plain text.

### Correctness criteria

- `[x]` `ProvidersConfig` parsing fails with a clear error if `base_url` is not valid (not a valid URL) — `base_url` is typed as `Url`, not as `String` (`domain/providers.rs:161`), so parsing fails before a `ProviderConfig` exists.
- `[x]` `ProviderRegistry::resolve(provider_id, Capability)` fails with `RegistryError::ProviderNotFound` if the ID does not exist (`domain/providers.rs:323`, test at `crates/webfang_core/src/domain/providers/tests.rs:81`).
- `[x]` `ProviderRegistry::resolve(provider_id, Capability::Embedding)` fails with `RegistryError::CapabilityMismatch` if the provider does not declare `Embedding` (`providers.rs:335`, test at `crates/webfang_core/src/domain/providers/tests.rs:90`).
- `[x]` `ProviderRegistry::resolve(provider_id, Capability::Completion)` fails with `RegistryError::CapabilityMismatch` if the provider does not declare `Completion` (same capability path).
- `[x]` `OpenAiCompatibleProvider` wraps `OpenAiLlmClient` and resolves the credential once at startup (`infrastructure/llm/provider.rs:107`, `new()` at `:126`).
- `[x]` `build_default_http_client()` produces the canonical config (Chrome145, 60s timeout, 10s connect, SSRF guard armed) — `infrastructure/llm/provider.rs:87-89` via the shared constructor `infrastructure/llm/http_client.rs`.

> `resolve()` returns `&ProviderConfig`, not a port: the adapter construction belongs to the binary layer (`crates/webfang_core/src/cli/llm_wire.rs`). See §5.

---

## Architecture notes

### On `EmbeddingPort` vs `EmbeddingCapable`

The audit of previous rounds correctly concluded that creating `EmbeddingCapable`/`CompletionCapable` as new traits was redundant: `EmbeddingPort` already covers the remote embeddings case (it has `embed_batch()`, `embedding_dim()`, `model_tag()`, and handles errors with `SemanticError` that distinguishes Transport/Auth/Status). Reusing `EmbeddingPort` for remote providers is the right decision.

The only reason to consider creating new traits was to give "capabilities" to a provider (embeddings, completions, both). But that is better solved with `ProviderConfig.capabilities: Vec<Capability>` in the config and with resolution methods in `ProviderRegistry` that verify the capability before returning the port.

### On `AnyProviderHandle`

Document 5 proposed `AnyProviderHandle` as an enum with variants `EmbeddingOnly`, `CompletionOnly`, `Both`. The document 6 audit pointed out that this was redundant against the slot-based design that document 5 itself had proposed two rounds earlier (`resolve(provider_id, Capability) -> Result<&ProviderConfig, RegistryError>`). The final design adopts slot-based resolution: there is no intermediate handle, the registry resolves directly by ID and by capability, and returns the config; the port is built by the binary layer.

### On shared `wreq::Client`

The scalability objection (many tenants → many connection pools) is valid in the long term, but the solution does not require changing the signature of `OpenAiLlmClient::new()`. `with_http(http, base_url, api_key)` is added, accepting an external `wreq::Client`. The registry can share clients between providers of the same `base_url` by creating a `wreq::Client` with `build_default_http_client()` and passing it to `with_http()`.

This is an additive extension, not a refactor of the existing client. `OpenAiLlmClient::new(base_url, api_key)` keeps working the same way, creating its own client internally. Only the new registry/provider code has the option to share.

### On the dependency inversion `AuthSource` → `ProvidersConfig`

Document 8's critique was correct: `AuthSource` is upstream of `ProvidersConfig`. The `ProvidersConfig` parser cannot be written without knowing which auth sources are supported, because that defines the `auth` schema in each provider config. That is why the implementation order is: first `AuthSource` + `AuthError` + `resolve()`, then `ProvidersConfig` + `ProviderRegistry`.

---

## Pending implementation

Real state of each point of the design, verified against the tree.

1. [x] **`AuthSource` + `AuthError` + `resolve()`** — `crates/webfang_core/src/domain/auth_source.rs` (`AuthError` `:20`, `AuthSource` `:58`, `resolve()` `:118`).
2. [x] **`DetectedStore::detect()`** — `crates/webfang_core/src/domain/local_secret_store.rs` (`DetectedStore` `:16`, `detect()` `:48`). The type this design called `LocalSecretStore` was implemented as `DetectedStore`; the old name remained only as a module name (see §7).
3. [x] **`build_default_http_client()`** extracted from `OpenAiLlmClient` — `crates/webfang_core/src/infrastructure/llm/provider.rs:87` (not in `infrastructure/http/`).
4. [x] **`OpenAiLlmClient::with_http(http, base_url, api_key)`** — `crates/webfang_core/src/infrastructure/llm/client.rs:167`.
5. [x] **`ProvidersConfig` + `ProviderRegistry`** — `crates/webfang_core/src/domain/providers.rs` (`ProvidersConfig` `:269`, `ProviderRegistry` `:300`); they live in `domain/`, not in `application/`.
6. [x] **`OpenAiCompatibleProvider`** — `crates/webfang_core/src/infrastructure/llm/provider.rs:107`.
7. [x] **Container integration** — the real wiring is in `crates/webfang_core/src/cli/llm_wire.rs:46` (`build_llm_provider`) and `:107` (`build_embedding_provider`), not in `container.rs`.
8. [x] **Integration tests** — `crates/webfang_core/tests/auth_source_none_test.rs` (`:35`, `:457`).
9. [ ] **Contract fixtures** (`crates/webfang_core/tests/fixtures/{openai,openrouter,freellm,groq}.json`) — kept by owner decision: cross-provider coverage remains necessary.

---

## References

- Existing traits: `EmbeddingPort` (`domain/embedding_port.rs`), `LlmPort` (`domain/llm_port.rs`).
- Existing implementations: `SemanticCleanerImpl` (`webfang_core/.../infrastructure/semantic_cleaner.rs`), `RemoteEmbeddingAdapter` (`webfang_core/.../remote.rs`), `OpenAiLlmClient` (`webfang_core/.../client.rs`).
- Existing credential management: `CredentialStore` + `ApiKey` (`domain/credentials.rs`).
- Embeddings namespacing design: `model_tag` and `embedding_dim` in `EmbeddingPort`, with validation in `vault_search.rs`.
- Error classification: `ScraperError::classify()` → `ErrorClass` (`error.rs`), with `RateLimited(u64)` for 429 and `Http { status }` for other codes.


---

## Known deleted implementation (from the worktree `feat/remote-ai-providers`)

> **Note:** This worktree was deleted and its commits are not physically recoverable (the branch was deleted with `git branch -D` and `git worktree remove`). However, the complete design is documented here based on the analysis of its 8 commits (~2000 lines).

### SemanticError::{Transport, Auth, Status} (commit b1104ef1)


### RemoteEmbeddingAdapter with a complete guard-chain (commit 5fcf8e0f, ~651 lines)

- Implements `EmbeddingPort` with:
  - `embed(text)` → an HTTP POST request to the endpoint with the encoded text, receives `EmbedResponse` with the vectors.
  - `embed_batch(texts)` → a single request with all texts (not one per text), decodes `Vec<Vec<f32>>` from the response.
  - **Complete guard-chain (THIS IS INHERITABLE and must be replicated in `OpenAiCompatibleProvider`):**
    1. **Timeout** — 60s request / 10s connect (configured in the `wreq` client)

### SSRF hardening per redirect hop (commits 594c53ea + 71564877 + a0198312, ~159 lines)

- **`resolve_and_validate(url) -> Result<(), ForbiddenResolution>`** (domain/ssrf_guard.rs, commit 594c53ea, +210 lines):
  - Pure function (resolver injected for deterministic testing).
  - If the host is an **IP literal** (IPv4 or IPv6), it parses and validates it against `is_forbidden_ip()` **without doing DNS** — rejects synchronously if private/loopback/link-local.
  - If the host is a **hostname**, it resolves A/AAAA records and validates **each record** against `is_forbidden_ip()`.
  - **Empty answers** → fail closed (reject).
  - **DNS errors** → fail closed (reject).
  - Only hostnames that resolve to public IPs pass.
  - This function is **injectable** (the real DNS resolver is passed from infrastructure) but the validation logic is pure and deterministic.
- **Per-hop redirect re-validation** (infrastructure/ssrf.rs, commit 71564877, +279 lines):
  - `redirect_policy()` moves from `domain/ssrf_guard.rs` to `infrastructure/ssrf.rs`.
  - **Each redirect target** (each 301/302/307/308 hop) goes through `resolve_and_validate()` before continuing.
  - If the redirect target is a forbidden IP literal → it stops at redirect time.
  - If the target is a hostname that resolves to a private IP → it stops.
  - The connect-time `ValidatingResolver` **remains armed** as a TOCTOU backstop.
  - The `secure_client()` composition does not change.
- **E2E test redirect-to-loopback-hostname** (ssrf_rfc1918_e2e_test.rs, commit a0198312, +62 lines):
  - Row 10 of the E2E test — a 302 with host `localhost` (or a hostname that resolves to loopback) stops at redirect time, produces a terminal error (exit 69), and the seed journal is the only record.
  - `resolve_and_validate()` is refactored into helpers `validate_literal()` (IP literals, no DNS) and `validate_answer_set()` (resolved hostnames, validates each record).

> **This block is THE most important to inherit:** without `resolve_and_validate()` and per-hop revalidation on redirects, `OpenAiCompatibleProvider` (Phase 3) **regresses in security** relative to the state that already existed — it reintroduces the TOCTOU window in redirects that was already closed. The SSRF hardening is already implemented in code that can be reimplemented identically from this document.

    2. **SSRF entry-check before each attempt** — `resolve_and_validate()` (see below) to validate that the endpoint is not a private/loopback/link-local IP
    3. **Retry classification** — exponential backoff 200ms → 2s, max 3 attempts, with cancellation by timeout
    4. **Body cap 50 MiB** — streaming read with `read_body_capped`, `BodyTooLarge` error if exceeded (real protection the new design must include from day 1)
  - Decodes `Vec<Vec<f32>>` from the response as little-endian `f32` BLOB.
  - Validates that the number of returned vectors matches the number of texts sent.
- **`embedding_dim()`** returns the model's declared dimension.
- **`model_tag()`** returns `"remote"` (or the configured override).

### VaultAiPorts with at-most-once injection (commit beef4388, ~84 lines)

- `Container::vault_ai_ports: Option<VaultAiPorts>`
- `VaultAiPorts::embedding_port: Option<Arc<dyn EmbeddingPort>>` — `None` by default
- `vault_ai_ports.with_remote_embedding(port)` — at-most-once injection (`OnceCell` semantics).
- **COUPLED TO THE OLD MODEL.** It must be reimplemented over `ProviderRegistry` in the new design; it is not recovered as-is.

### Vector namespacing by model_tag (commit 34ecc730, ~317 lines)

- `NoteChunkVector` has a `model_tag: String` field with `#[serde(default = "default_model_tag")]` that returns `LEGACY_MODEL_TAG = "granite-97m-local"` for legacy rows without a tag.
- **`ADD COLUMN model_tag` in SQLite** with lazy backfill (existing rows are read as legacy, not rewritten).
- When indexing a note: `index_note_stamps_embedder_model_tag` — each chunk vector is written with the `model_tag` of the embedder that generated it (extracted from `EmbeddingPort::model_tag()`).
- **`search_skips_dimension_mismatched_vectors`** — in `vault_search.rs`, if the query vector has a dimension that does not match the stored vectors of that `model_tag`, they are skipped (not silently scored 0.0).
- **THIS IS INHERITABLE AND ALREADY IMPLEMENTED.** The `model_tag` column, the lazy backfill, and the cross-dim rejection already exist in the vault store.

- `Transport(String)` — `TransientRetriable` classification. Temporary network failures: TCP reset, request/connect timeout, transient DNS. Retried with backoff.
- `Auth(String)` — `PermanentFatal` classification. 401/403 responses from the remote endpoint (unknown, missing, or revoked key). Not retried — the credential is not valid.
- `Status { status: u16, detail: String }` — depends on the code: 429 → `TransientBackoff` (backoff with `Retry-After`), 5xx → `TransientRetriable` (exponential), the rest → terminal.
- These variants feed the remote adapter's error classification to decide whether to retry or fail terminally.

### RemoteEmbeddingConfig::from_env (commit 739b0d67)

- Configuration for the adapter: `endpoint: Url`, `api_key: ApiKey`, Bearer on each request.
- Redacted `Debug`: the API key **never appears in logs, traces or error messages** (explicit criterion of issue #1462).
- Configurable `model_tag`, defaulting to `"remote"` if not overridden.
- **COUPLED TO THE OLD CREDENTIALS MODEL.** It must be reimplemented over `AuthSource` in the new design; it is not recovered as-is.

---

## Claim manifest

This doc is a **normative** spec: 13 code files cite it. Since it lives outside the mdBook book, no gate read it, and a signature that did not compile (`AuthSource::resolve()` documented as `Result<ApiKey, AuthError>` when the real one is `Result<Option<ApiKey>, AuthError>`) survived for months (#2136).

The block below is the **manifest**: each normative claim this doc makes about the code, with the exact line that backs it. It is verified by `scripts/check_ai_providers_doc.sh`, which fails closed on any drift.

Entry types:

- `claim` — the cited line contains the expected substring (literal match).
- `shape` — the type declared on that line has **exactly** the expected members (variants for `enum`, fields for `struct`), in both directions.
- `impl` — the doc fence containing that declaration is contained in the cited file.
- `absent` — the symbol **does not** appear in the file (or in all of `crates/`, if the file is `*`).

<!-- claims:begin -->
```claims
# --- §1 AuthSource · §2 AuthError · §3 resolve() -----------------------------
shape  | crates/webfang_core/src/domain/auth_source.rs | 58 | pub enum AuthSource { | Keyring,EncryptedFile,Env,None
claim  | crates/webfang_core/src/domain/auth_source.rs | 102 | None,
shape  | crates/webfang_core/src/domain/auth_source.rs | 20 | pub enum AuthError { | KeyringUnavailable,CredentialNotFound,KeyringFailure,FileNotFound,FileRead,Decrypt,FileFormat,EnvNotSet,Invalid
impl   | crates/webfang_core/src/domain/auth_source.rs | 118 | pub fn resolve(&self) -> Result<Option<ApiKey>, AuthError> {
claim  | crates/webfang_core/src/domain/auth_source.rs | 151 | fn resolve_keyring(service: &str, account: &str) -> Result<ApiKey, AuthError> {
claim  | crates/webfang_core/src/domain/auth_source.rs | 72 | Credencial en variable de entorno (legacy, solo CI o desarrollo)
claim  | crates/webfang_core/src/domain/auth_source.rs | 6 | Sin variante `Inline` en v1
claim  | crates/webfang_core/src/domain/auth_source.rs | 125 | }
# --- §4 ProvidersConfig / ProviderConfig / ProviderKind / Capability ----------
shape  | crates/webfang_core/src/domain/providers.rs | 152 | pub struct ProviderConfig { | id,display_name,kind,base_url,auth,capabilities,model,embedding_dim,allow_loopback,network_policy,proxy,extra_body,query_extra_body,send_encoding_format
impl   | crates/webfang_core/src/domain/providers.rs | 152 | pub struct ProviderConfig {
shape  | crates/webfang_core/src/domain/providers.rs | 269 | pub struct ProvidersConfig { | providers
shape  | crates/webfang_core/src/domain/providers.rs | 43 | pub enum ProviderKind { | OpenAiCompatible
shape  | crates/webfang_core/src/domain/providers.rs | 55 | pub enum Capability { | Embedding,Completion
claim  | crates/webfang_core/src/domain/providers.rs | 161 | pub base_url: Url,
claim  | crates/webfang_core/src/domain/providers.rs | 165 | pub capabilities: Vec<Capability>,
claim  | crates/webfang_core/src/domain/providers.rs | 98 | "encoding_format",
# --- §5 ProviderRegistry ------------------------------------------------------
shape  | crates/webfang_core/src/domain/providers.rs | 300 | pub struct ProviderRegistry { | config
impl   | crates/webfang_core/src/domain/providers.rs | 300 | pub struct ProviderRegistry {
impl   | crates/webfang_core/src/domain/providers.rs | 323 | pub fn resolve(
claim  | crates/webfang_core/src/domain/providers.rs | 377 | pub fn resolve_default_completion(&self) -> Result<&ProviderConfig, RegistryError> {
claim  | crates/webfang_core/src/domain/providers.rs | 395 | pub fn resolve_default_embedding(&self) -> Result<&ProviderConfig, RegistryError> {
claim  | crates/webfang_core/src/domain/providers.rs | 67 | ProviderNotFound {
claim  | crates/webfang_core/src/domain/providers.rs | 73 | CapabilityMismatch {
claim  | crates/webfang_core/src/domain/providers.rs | 335 | RegistryError::CapabilityMismatch {
claim  | crates/webfang_core/src/domain/providers/tests.rs | 81 | fn resolve_unknown_id_is_provider_not_found() {
claim  | crates/webfang_core/src/domain/providers/tests.rs | 90 | fn resolve_missing_capability_is_mismatch() {
# --- §6 TenantAiContext (no implementado) -------------------------------------
absent | * | - | TenantAiContext
# --- §7 DetectedStore ---------------------------------------------------------
shape  | crates/webfang_core/src/domain/local_secret_store.rs | 16 | pub struct DetectedStore { | keyring_available,encrypted_file_path,env_vars_detected,recommendation
impl   | crates/webfang_core/src/domain/local_secret_store.rs | 16 | pub struct DetectedStore {
claim  | crates/webfang_core/src/domain/local_secret_store.rs | 37 | pub recommendation: AuthSource,
claim  | crates/webfang_core/src/domain/local_secret_store.rs | 48 | pub fn detect() -> Self {
# --- wiring binario ----------------------------------------------------------
claim  | crates/webfang_core/src/cli/llm_wire.rs | 46 | pub fn build_llm_provider(
claim  | crates/webfang_core/src/cli/llm_wire.rs | 107 | pub async fn build_embedding_provider(
claim  | crates/webfang_core/src/cli/llm_wire.rs | 74 | registry.resolve(id, Capability::Embedding)
claim  | crates/webfang_core/src/cli/llm_wire.rs | 80 | registry.resolve_default_embedding()
claim  | crates/webfang_core/src/cli/commands/doctor.rs | 63 | registry.resolve(id, Capability::Embedding)
claim  | crates/webfang_core/src/cli/commands/doctor.rs | 78 | registry.resolve_default_embedding()
claim  | crates/webfang_core/src/application/vault_search.rs | 70 | embedding: Arc<dyn EmbeddingPort>,
# --- infraestructura LLM -----------------------------------------------------
claim  | crates/webfang_core/src/infrastructure/llm/provider.rs | 107 | pub struct OpenAiCompatibleProvider {
claim  | crates/webfang_core/src/infrastructure/llm/provider.rs | 126 | pub fn new(config: ProviderConfig) -> Result<Self, ProviderInitError> {
claim  | crates/webfang_core/src/infrastructure/llm/provider.rs | 87 | pub fn build_default_http_client() -> Result<wreq::Client, ProviderInitError> {
claim  | crates/webfang_core/src/infrastructure/llm/provider.rs | 89 | }
claim  | crates/webfang_core/src/infrastructure/llm/client.rs | 167 | pub fn with_http(client: wreq::Client, base_url: Url, api_key: ApiKey) -> Self {
# --- tests de integración -----------------------------------------------------
claim  | crates/webfang_core/tests/auth_source_none_test.rs | 35 | OpenAiCompatibleProvider
claim  | crates/webfang_core/tests/auth_source_none_test.rs | 457 | OpenAiCompatibleProvider::new(config)
```
<!-- claims:end -->