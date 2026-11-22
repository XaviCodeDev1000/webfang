use super::fixtures::*;
use super::*;

#[test]
fn write_provider_appends_a_parseable_record() {
    let (_dir, path) = temp_config();
    let provider = test_provider("connect-a1b2");

    write_provider(&path, &provider).expect("append record");

    // The file must parse back through the SAME loader the run uses.
    let loaded = crate::cli::config::ConfigDefaults::load_explicit(&path)
        .expect("written config must parse");
    assert_eq!(loaded.providers.len(), 1);
    let written = &loaded.providers[0];
    assert_eq!(written.id, "connect-a1b2");
    assert_eq!(written.model.as_deref(), Some("nomic-embed-text"));
    assert!(written.has_capability(Capability::Embedding));
}

/// A first run has no `webfang/` directory yet — the write must create it
/// rather than fail after the endpoint probe already succeeded.
#[test]
fn write_provider_creates_a_missing_parent_directory() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("webfang").join("config.toml");
    let provider = test_provider("connect-fresh");

    write_provider(&path, &provider).expect("the parent dir is created on demand");
    assert!(path.exists());
}

/// Re-registering the same id must replace the record, never append a
/// second one: `ProvidersConfig::find` is first-match, so a duplicate
/// would let the stale endpoint shadow the newly connected one forever.
#[test]
fn write_provider_replaces_an_existing_id_instead_of_duplicating() {
    let (_dir, path) = temp_config();
    let mut provider = test_provider("connect-a1b2");
    provider.base_url = url::Url::parse("https://old.example.test/v1").expect("url");

    write_provider(&path, &provider).expect("first write");
    // Same endpoint, refreshed model: the id is what must stay unique, and
    // a different endpoint under the same id is now refused outright (see
    // `write_provider_refuses_to_carry_operator_fields_across_endpoints`).
    provider.model = Some("nomic-embed-text-v2".into());
    write_provider(&path, &provider).expect("second write");

    let loaded =
        crate::cli::config::ConfigDefaults::load_explicit(&path).expect("config must parse");
    assert_eq!(
        loaded.providers.len(),
        1,
        "re-registering the same id must not append a duplicate"
    );
    assert_eq!(
        loaded.providers[0].model.as_deref(),
        Some("nomic-embed-text-v2"),
        "the record must be the one just connected"
    );
}

/// THE regression test for the data-loss bug: a real config holds far more
/// than `[[providers]]`, and `ProvidersConfig` models exactly one field.
/// Writing a provider must leave every other key, table and comment
/// byte-for-byte intact, or the operator loses their whole config on the
/// first `connect`.
#[test]
fn write_provider_preserves_unrelated_config_keys_and_comments() {
    let (_dir, path) = temp_config();
    let original = "\
# Configuración de webfang — no borrar a mano
format = \"markdown\"
concurrency = \"auto\"
max_pages = 25
vault_path = \"/home/op/vault\"
log_level = \"debug\"

[[model_profiles.profiles]]
id = \"granite-97m\"
chars_per_token = 4.0
matryoshka = true

[[providers]]
id = \"openai\"
display_name = \"OpenAI\"
kind = \"open_ai_compatible\"
base_url = \"https://api.openai.com/v1\"
auth = { source = \"none\" }
capabilities = [\"embedding\"]
model = \"text-embedding-3-small\"
allow_loopback = false
";
    seed_config(&path, original);

    let provider = test_provider("connect-a1b2");
    write_provider(&path, &provider).expect("append record");

    let written = std::fs::read_to_string(&path).expect("read back");

    // Every unrelated top-level key survives.
    for key in [
        "format = \"markdown\"",
        "concurrency = \"auto\"",
        "max_pages = 25",
        "vault_path = \"/home/op/vault\"",
        "log_level = \"debug\"",
    ] {
        assert!(
            written.contains(key),
            "lost unrelated key: {key}\n---\n{written}"
        );
    }
    // The comment survives (a typed round-trip drops it entirely).
    assert!(
        written.contains("# Configuración de webfang — no borrar a mano"),
        "the operator's comment was destroyed\n---\n{written}"
    );
    // The unrelated `[[model_profiles]]` table survives.
    assert!(
        written.contains("[[model_profiles.profiles]]") && written.contains("matryoshka = true"),
        "lost the model_profiles table\n---\n{written}"
    );
    // The pre-existing provider AND the new one are both present.
    assert!(
        written.contains("id = \"openai\""),
        "lost the existing provider"
    );
    assert!(
        written.contains("id = \"connect-a1b2\""),
        "lost the new provider"
    );

    // And the file still loads through the real loader.
    let loaded = crate::cli::config::ConfigDefaults::load_explicit(&path)
        .expect("written config must still parse");
    assert_eq!(loaded.providers.len(), 2, "both providers must survive");
}

/// Reconnecting the SAME endpoint must NOT clobber hand-tuned operator
/// fields: an existing provider with credentials / loopback / a pinned dim
/// would be silently downgraded to the anonymous, loopback-forbidden shape
/// `connect` builds.
///
/// The endpoint has to be identical — see
/// [`write_provider_refuses_to_carry_operator_fields_across_endpoints`] for
/// the refusal that guards the other direction.
#[test]
fn write_provider_preserves_operator_fields_when_reconnecting_the_same_endpoint() {
    let (_dir, path) = temp_config();
    let seeded = "\
[[providers]]
id = \"connect-a1b2\"
display_name = \"Viejo\"
kind = \"open_ai_compatible\"
base_url = \"https://api.example.test/v1\"
auth = { source = \"keyring\", service = \"webfang\", account = \"op\" }
capabilities = [\"embedding\"]
model = \"nomic-embed-text\"
embedding_dim = 768
allow_loopback = true
network_policy = { mode = \"allowlist\", allow_cidrs = [\"10.0.0.0/8\"], allow_hosts = [\"vllm.lan\"] }
";
    seed_config(&path, seeded);

    // What `connect` would produce: anonymous, loopback forbidden, no dim.
    let mut provider = test_provider("connect-a1b2");
    provider.model = Some("nomic-embed-text-v2".into());
    write_provider(&path, &provider).expect("merge record");

    let loaded =
        crate::cli::config::ConfigDefaults::load_explicit(&path).expect("config must parse");
    assert_eq!(loaded.providers.len(), 1, "must not duplicate the id");
    let written = &loaded.providers[0];

    // Refreshed by the probe.
    assert_eq!(written.base_url.as_str(), "https://api.example.test/v1");
    assert_eq!(written.display_name, "Connect A1b2");
    assert_eq!(written.model.as_deref(), Some("nomic-embed-text-v2"));

    // Preserved operator intent.
    assert!(written.allow_loopback, "loopback opt-in was clobbered");
    assert_eq!(
        written.embedding_dim,
        Some(768),
        "the operator's pinned dim was clobbered"
    );
    assert!(
        !matches!(written.auth, AuthSource::None),
        "credentials were clobbered by the anonymous connect shape"
    );
    assert_eq!(
        written.network_policy.allow_hosts,
        vec!["vllm.lan".to_string()],
        "the operator's LAN allowlist was clobbered"
    );
}

/// THE cross-endpoint credential bug: `connect` accepts an explicit
/// `--id`, so an operator can point an id that already carries one host's
/// credentials and LAN allowlist at a completely different host. Carrying
/// the operator-owned fields across would ship `old.host`'s keyring entry
/// to `new.host` while `register` prints "conectado y registrado". The
/// write fails closed and the file is untouched.
#[test]
fn write_provider_refuses_to_carry_operator_fields_across_endpoints() {
    let (_dir, path) = temp_config();
    let seeded = "\
[[providers]]
id = \"openai\"
display_name = \"OpenAI viejo\"
kind = \"open_ai_compatible\"
base_url = \"https://old.host/v1\"
auth = { source = \"keyring\", service = \"webfang\", account = \"op\" }
capabilities = [\"embedding\"]
model = \"text-embedding-3-small\"
embedding_dim = 1024
network_policy = { mode = \"allowlist\", allow_cidrs = [\"10.0.0.0/8\"], allow_hosts = [\"old.host\"] }
";
    seed_config(&path, seeded);
    let before = std::fs::read(&path).expect("read seed");

    let mut provider = test_provider("openai");
    provider.display_name = "OpenAI".into();
    provider.base_url = url::Url::parse("https://new.host/v1").expect("url");
    provider.model = Some("text-embedding-3-small".into());

    let err = write_provider(&path, &provider)
        .expect_err("credentials must never cross to another endpoint");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    let message = err.to_string();
    assert!(message.contains("openai"), "must name the id: {message}");
    assert!(
        message.contains("https://old.host/v1") && message.contains("https://new.host/v1"),
        "must name both endpoints: {message}"
    );
    assert!(
        message.contains("--id"),
        "must tell the operator how to proceed: {message}"
    );

    assert_eq!(
        before,
        std::fs::read(&path).expect("read back"),
        "a refused merge must leave the config byte-for-byte identical"
    );
}

/// Comments the operator wrote INSIDE the provider block are part of their
/// hand-tuning, and `Table::insert` on an occupied entry drops both the key
/// decor (a preceding `# comment` line) and the value decor (a trailing
/// `# comment`). The merge must carry both onto the refreshed value.
#[test]
fn write_provider_preserves_comments_inside_the_merged_provider_block() {
    let (_dir, path) = temp_config();
    let seeded = "\
[[providers]]
id = \"connect-a1b2\"
# etiqueta que el operador eligió a mano
display_name = \"Viejo\"
# backend validado contra el servidor de embeddings
kind = \"open_ai_compatible\"
base_url = \"https://api.example.test/v1\"  # no cambiar sin avisar al equipo
auth = { source = \"none\" }
capabilities = [\"embedding\"]
# modelo aprobado por Ticket WEB-482
model = \"nomic-embed-text\"
";
    seed_config(&path, seeded);

    let mut provider = test_provider("connect-a1b2");
    provider.model = Some("nomic-embed-text-v2".into());
    write_provider(&path, &provider).expect("merge record");

    let written = std::fs::read_to_string(&path).expect("read back");
    for comment in [
        "# etiqueta que el operador eligió a mano",
        "# backend validado contra el servidor de embeddings",
        "# no cambiar sin avisar al equipo",
        "# modelo aprobado por Ticket WEB-482",
    ] {
        assert!(
            written.contains(comment),
            "the merge destroyed a comment inside the provider block: {comment}\n---\n{written}"
        );
    }
}

/// A config that is well-formed TOML but type-invalid against the
/// `ProvidersConfig` shape (`base_url = 42`) used to sail through connect
/// untouched: `write_provider` reported success, printed "conectado y
/// registrado", and every later run silently resolved ZERO providers via
/// the documented fallback. A loud refusal is the only honest outcome.
#[test]
fn write_provider_refuses_a_config_whose_providers_are_type_invalid() {
    let (_dir, path) = temp_config();
    let seeded = "\
[[providers]]
id = \"connect-a1b2\"
display_name = \"Connect A1b2\"
kind = \"open_ai_compatible\"
base_url = \"https://api.example.test/v1\"
auth = { source = \"none\" }
capabilities = [\"embedding\"]
model = \"nomic-embed-text\"

[[providers]]
id = \"otro\"
display_name = \"Otro\"
kind = \"open_ai_compatible\"
base_url = 42
auth = { source = \"none\" }
capabilities = [\"embedding\"]
model = \"nomic-embed-text\"
";
    seed_config(&path, seeded);
    let before = std::fs::read(&path).expect("read seed");

    let provider = test_provider("connect-a1b2");

    let err = write_provider(&path, &provider)
        .expect_err("a type-invalid config must not be written through");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    let message = err.to_string();
    assert!(
        message.contains("config.toml"),
        "the error must name the file: {message}"
    );

    assert_eq!(
        before,
        std::fs::read(&path).expect("read back"),
        "a refused write must leave the config byte-for-byte identical"
    );
}

/// The permission the full-document gate buys its honesty with: keys the
/// `ConfigDefaults` struct does NOT model — an `[obsidian]` table, a
/// future key — but that are well typed must keep passing, or the gate
/// narrows the config language connect is allowed to edit. `format` is
/// modeled; `[obsidian]` (the struct models the flat `obsidian_*` fields,
/// not a table) is not. Both must survive the write.
#[test]
fn write_provider_accepts_well_typed_keys_the_config_does_not_model() {
    let (_dir, path) = temp_config();
    let seeded = "\
format = \"json\"

# Sección que ConfigDefaults no modela — debe sobrevivir igual.
[obsidian]
vault = \"/home/op/notes\"

[[providers]]
id = \"existing\"
display_name = \"Existing\"
kind = \"open_ai_compatible\"
base_url = \"https://api.example.test/v1\"
auth = { source = \"none\" }
capabilities = [\"embedding\"]
model = \"nomic-embed-text\"
";
    seed_config(&path, seeded);

    let provider = test_provider("connect-a1b2");
    write_provider(&path, &provider).expect("well-typed unmodeled keys must pass the gate");

    let written = std::fs::read_to_string(&path).expect("read back");
    assert!(
        written.contains("format = \"json\""),
        "the modeled key was lost\n---\n{written}"
    );
    assert!(
        written.contains("[obsidian]") && written.contains("vault = \"/home/op/notes\""),
        "an unmodeled but well-typed section was refused or lost\n---\n{written}"
    );

    let loaded = crate::cli::config::ConfigDefaults::load_explicit(&path)
        .expect("written config must parse through the real loader");
    assert_eq!(loaded.providers.len(), 2, "both providers must survive");
}

/// `providers = [ { … } ]` is legal TOML, deserializes through
/// `ProvidersConfig { providers: Vec<ProviderConfig> }` and is accepted by
/// the run — so `connect` must edit it, not refuse it.
///
/// Note the seed carries EVERY required `ProviderConfig` field: the A1
/// validation gate runs first, and `display_name` / `kind` / `auth` have no
/// default, so a partial inline record is refused before it is ever
/// converted.
#[test]
fn write_provider_edits_an_inline_providers_array() {
    let (_dir, path) = temp_config();
    let seeded = "\
format = \"markdown\"
# declarados en línea; connect los reescribe como [[providers]]
providers = [ { id = \"old\", display_name = \"Old\", kind = \"open_ai_compatible\", \
base_url = \"https://old.example.test/v1\", auth = { source = \"none\" }, \
model = \"m\", capabilities = [\"embedding\"] } ]
";
    seed_config(&path, seeded);

    let provider = test_provider("connect-a1b2");
    write_provider(&path, &provider).expect("an inline array is editable");

    let written = std::fs::read_to_string(&path).expect("read back");
    assert!(
        written.contains("format = \"markdown\""),
        "an unrelated top-level key was lost\n---\n{written}"
    );
    assert!(
        !written.contains("providers = ["),
        "the inline array should have been converted to [[providers]]\n---\n{written}"
    );
    assert!(
        written.contains("# declarados en línea; connect los reescribe como [[providers]]"),
        "the conversion dropped the comment above the `providers` key\n---\n{written}"
    );

    let loaded = crate::cli::config::ConfigDefaults::load_explicit(&path)
        .expect("written config must parse through the real loader");
    assert_eq!(loaded.providers.len(), 2, "both providers must survive");
    assert_eq!(loaded.providers[0].id, "old");
    assert_eq!(loaded.providers[1].id, "connect-a1b2");
}

/// Reconnecting must not narrow an operator's widened `capabilities`:
/// `connect` only knows about embeddings, but the provider may have been
/// hand-extended, and replacing the list would drop the extra capability.
#[test]
fn write_provider_unions_capabilities_instead_of_replacing_them() {
    let (_dir, path) = temp_config();
    seed_config(
        &path,
        "[[providers]]\nid = \"connect-a1b2\"\ndisplay_name = \"Wide\"\n\
         kind = \"open_ai_compatible\"\nbase_url = \"https://api.example.test/v1\"\n\
         auth = { source = \"none\" }\ncapabilities = [\"embedding\", \"completion\"]\n\
         model = \"nomic-embed-text\"\nallow_loopback = false\n",
    );

    let provider = test_provider("connect-a1b2");
    write_provider(&path, &provider).expect("merge record");

    let loaded =
        crate::cli::config::ConfigDefaults::load_explicit(&path).expect("config must parse");
    let written = &loaded.providers[0];
    assert!(
        written.has_capability(Capability::Embedding),
        "embedding must remain"
    );
    assert!(
        written.has_capability(Capability::Completion),
        "the operator's extra capability was dropped by the merge"
    );
}
