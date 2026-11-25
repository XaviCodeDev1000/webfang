use crate::domain::providers::{ProviderConfig, RedactedUrl};

/// Splice `provider` into the `providers` array of `document`, in place.
///
/// An existing entry with the same id is merged field by field (never
/// replaced wholesale); see [`merge_provider`] for the exact split. A plain
/// overwrite would silently downgrade a working provider to the anonymous,
/// loopback-forbidden shape `connect` builds.
pub(super) fn upsert_provider(
    document: &mut toml_edit::DocumentMut,
    provider: &ProviderConfig,
) -> Result<(), std::io::Error> {
    let mut entry = toml_edit::ser::to_document(provider)
        .map_err(|e| {
            std::io::Error::other(format!(
                "no se pudo serializar el provider '{}': {e}",
                provider.id
            ))
        })?
        .as_table()
        .clone();

    // The config file is the source of truth for the endpoint. `ProviderConfig`'s
    // `Serialize` hook redacts userinfo for OUTPUT sinks (`--trace-file`, debug
    // dumps), so the redacted text must never be written back: re-insert the
    // configured URL verbatim or a re-`connect` silently drops the credential.
    entry.insert("base_url", toml_edit::value(provider.base_url.as_str()));

    // `providers` missing, wrong type, or empty: start a fresh array of
    // tables. `entry` is an inline-shaped table from serde; `to_string` is
    // what turns it into a `[[providers]]` table, so store the serialized
    // form and let the writer render it.
    let array = ensure_providers_array(document)?;

    // Locate by id, then merge OR append in one pass without holding a
    // mutable borrow across both arms. `iter_mut().find` hands back the
    // mutable table directly — no index round-trip, no `expect()` — and the
    // append path is taken only when the find yielded nothing, so the two
    // borrows never overlap.
    let matched = array
        .iter_mut()
        .find(|table| table.get("id").and_then(|id| id.as_str()) == Some(provider.id.as_str()));
    if let Some(existing) = matched {
        merge_provider(existing, &entry)?;
        return Ok(());
    }
    array.push(entry);

    Ok(())
}

/// Borrow (creating if needed) the `providers` array-of-tables.
fn ensure_providers_array(
    document: &mut toml_edit::DocumentMut,
) -> Result<&mut toml_edit::ArrayOfTables, std::io::Error> {
    if !document.contains_key("providers") {
        document["providers"] = toml_edit::Item::ArrayOfTables(toml_edit::ArrayOfTables::new());
    }
    if document["providers"].as_array_of_tables_mut().is_none() {
        convert_inline_providers(document)?;
    }
    document["providers"]
        .as_array_of_tables_mut()
        .ok_or_else(providers_shape_error)
}

/// Rewrite an inline `providers = [ { … } ]` as a `[[providers]]` array of
/// tables so the rest of the write can use one shape.
///
/// The inline array is legal TOML, deserializes through `ProvidersConfig {
/// providers: Vec<ProviderConfig> }` and is accepted by the run, so refusing
/// it would be a narrowing nothing required. `Item::into_array_of_tables`
/// does the conversion for a non-empty all-inline-table array; an EMPTY
/// `providers = []` is the same declaration as no providers at all, and
/// `into_array_of_tables` declines it, so a fresh array-of-tables is built
/// there. Everything else keeps the loud refusal.
fn convert_inline_providers(document: &mut toml_edit::DocumentMut) -> Result<(), std::io::Error> {
    use toml_edit::{ArrayOfTables, Item, Value};

    let Some((key, item)) = document
        .get_key_value("providers")
        .map(|(k, v)| (k.clone(), v.clone()))
    else {
        return Err(providers_shape_error());
    };
    let key_prefix = key.leaf_decor().prefix().cloned();

    let mut item = match item.into_array_of_tables() {
        Ok(array_of_tables) => Item::ArrayOfTables(array_of_tables),
        Err(Item::Value(Value::Array(array))) if array.is_empty() => {
            Item::ArrayOfTables(ArrayOfTables::new())
        },
        Err(_) => return Err(providers_shape_error()),
    };

    // A `# comment` the operator wrote above `providers = [...]` lives in the
    // KEY's decor prefix. An array-of-tables renders its header with DEFAULT
    // key decor (`encode_key_path(..., DEFAULT_KEY_PATH_DECOR)`), so the key is
    // re-inserted with `fmt()`'d decor — carrying the original would splice the
    // comment INTO the header and emit `[[# comment\nproviders ]]`, a file
    // nothing can parse. The first converted table's OWN decor prefix renders
    // immediately before that header (`table.decor.prefix_encode` in
    // `visit_table`), which is the faithful home for the text. An empty array
    // has no table to carry it and is about to receive the appended record
    // anyway, so nothing is emitted for it.
    if let Some(prefix) = key_prefix {
        if let Some(first) = item
            .as_array_of_tables_mut()
            .and_then(|array_of_tables| array_of_tables.iter_mut().next())
        {
            first.decor_mut().set_prefix(prefix);
        }
    }

    document.insert("providers", item);
    Ok(())
}

/// The single loud refusal for a `providers` key this code cannot edit.
fn providers_shape_error() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        "'providers' existe en la configuración pero no es un array de tablas \
         ([[providers]]) ni un array de tablas en línea; no se toca el archivo \
         para no corromperlo",
    )
}

/// Merge the probed `incoming` record into an existing `[[providers]]` table.
///
/// Three groups, keyed on what `connect` is authoritative about:
///
/// 1. **Probe-owned** — `kind`, `base_url`, `model`, `display_name` — always
///    refreshed: they describe the endpoint the probe just validated, and that
///    is the whole point of re-running `connect`.
/// 2. **Operator-owned** — `auth`, `embedding_dim`, `allow_loopback`,
///    `network_policy` — never overwritten, and carried over only when the
///    record already points at the SAME endpoint `connect` just probed (see
///    [`same_endpoint`]). Preservation happens by NOT TOUCHING the key, so a
///    field `connect` cannot express is kept: `to_document` skips `None`, so
///    `embedding_dim` is never in `incoming` and the operator's pin survives by
///    absence, not by any insert rule. The keys `connect` DOES build —
///    `auth`, `allow_loopback`, `network_policy` — are filled in on absence.
/// 3. **Additive** — `capabilities` — the operator may have widened an
///    existing provider to serve more than embeddings, so the incoming
///    embedding capability is unioned in rather than replacing the list.
fn merge_provider(
    existing: &mut toml_edit::Table,
    incoming: &toml_edit::Table,
) -> Result<(), std::io::Error> {
    // A record declaring operator-owned fields describes ONE host: its
    // credentials, its LAN allowlist and its pinned dim are only meaningful
    // against the `base_url` they were written for. `connect` accepts an
    // explicit `--id`, so without this guard one host's keyring entry and
    // allowlist would be shipped to a different host while `register` printed
    // "conectado y registrado" — and the stale `embedding_dim` would fail the
    // next run closed. Checked BEFORE any mutation, so a refusal leaves nothing
    // half-merged and nothing written.
    if carries_operator_fields(existing) && !same_endpoint(existing, incoming) {
        return Err(cross_endpoint_error(existing, incoming));
    }

    // Group 1: probe-owned, always refreshed (decor carried over).
    for key in ["display_name", "base_url", "model", "kind"] {
        refresh_key(existing, key, incoming);
    }

    // Group 2: operator-owned, filled in only on absence.
    for key in ["auth", "embedding_dim", "allow_loopback", "network_policy"] {
        if !existing.contains_key(key) {
            if let Some(value) = incoming.get(key) {
                existing.insert(key, value.clone());
            }
        }
    }

    // Group 3: capabilities are unioned, never replaced.
    if let Some(value) = incoming.get("capabilities") {
        merge_capabilities(existing, value);
    }
    Ok(())
}

/// Overwrite one probe-owned key, carrying the operator's decor onto the
/// incoming value.
///
/// `Table::insert` on an Occupied entry calls `entry.key_mut().fmt()` and
/// `mem::replace`s the item, which drops BOTH the key decor (a `# comment`
/// line above the key) and the value decor (a trailing `# comment`). A plain
/// rewrite therefore deletes hand-written notes inside the provider block on
/// every re-`connect`. So read the decor that is actually on the old entry,
/// apply it to the incoming value, and re-insert under the ORIGINAL key.
fn refresh_key(existing: &mut toml_edit::Table, key: &str, incoming: &toml_edit::Table) {
    let Some(value) = incoming.get(key) else {
        return;
    };
    let Some((existing_key, existing_item)) = existing
        .get_key_value(key)
        .map(|(k, v)| (k.clone(), v.clone()))
    else {
        // Vacant: there is no old decor to carry, and a new key renders with
        // the default decor.
        existing.insert(key, value.clone());
        return;
    };

    let mut item = value.clone();
    if let Some(old) = existing_item.as_value() {
        if let Some(new) = item.as_value_mut() {
            *new.decor_mut() = old.decor().clone();
        }
    }
    existing.insert_formatted(&existing_key, item);
}

/// Whether the existing record declares any operator-owned field a merge would
/// have to carry across to the probed endpoint.
fn carries_operator_fields(existing: &toml_edit::Table) -> bool {
    ["auth", "embedding_dim", "allow_loopback", "network_policy"]
        .iter()
        .any(|key| existing.contains_key(key))
}

/// Whether the existing record points at the endpoint `connect` just probed.
///
/// An absent or unreadable `base_url` is NOT evidence of the probed endpoint,
/// and this predicate fails closed on it.
fn same_endpoint(existing: &toml_edit::Table, incoming: &toml_edit::Table) -> bool {
    match (
        existing.get("base_url").and_then(toml_edit::Item::as_str),
        incoming.get("base_url").and_then(toml_edit::Item::as_str),
    ) {
        (Some(existing_url), Some(incoming_url)) => existing_url == incoming_url,
        _ => false,
    }
}

/// The refusal for "these credentials belong to a different host".
fn cross_endpoint_error(
    existing: &toml_edit::Table,
    incoming: &toml_edit::Table,
) -> std::io::Error {
    const UNREADABLE: &str = "(sin base_url legible)";
    let id = incoming
        .get("id")
        .and_then(toml_edit::Item::as_str)
        .unwrap_or(UNREADABLE);
    // Both URLs are rendered without userinfo: this error reaches stderr, and a
    // configured endpoint may carry inline credentials.
    let redacted = |item: Option<&toml_edit::Item>| {
        item.and_then(toml_edit::Item::as_str)
            .map(|url| RedactedUrl::from_str_lossy(url).to_string())
            .unwrap_or_else(|| UNREADABLE.to_string())
    };
    let existing_url = redacted(existing.get("base_url"));
    let incoming_url = redacted(incoming.get("base_url"));

    std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!(
            "el provider '{id}' ya está registrado contra '{existing_url}' y el endpoint \
             conectado es '{incoming_url}': no se arrastran credenciales ni políticas de \
             red entre hosts distintos (no se toca el archivo). Usá un `--id` distinto, \
             o editá o borrá a mano el registro existente"
        ),
    )
}

/// Union an `incoming` capabilities array into `existing`'s, preserving order
/// and without duplicating an entry the operator already had.
fn merge_capabilities(existing: &mut toml_edit::Table, incoming: &toml_edit::Item) {
    let Some(incoming_caps) = incoming.as_array() else {
        return;
    };
    // Ensure the target is an array; if the operator wrote a non-array, leave
    // their value untouched rather than silently reshaping it.
    if existing.get("capabilities").is_some() && existing["capabilities"].as_array().is_none() {
        return;
    }
    let target = existing.entry("capabilities").or_insert_with(|| {
        toml_edit::Item::Value(toml_edit::Value::Array(toml_edit::Array::new()))
    });
    let Some(target) = target.as_array_mut() else {
        return;
    };
    for cap in incoming_caps {
        if !target.iter().any(|c| c.as_str() == cap.as_str()) {
            target.push(cap.clone());
        }
    }
}
