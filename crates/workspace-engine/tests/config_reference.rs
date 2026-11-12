//! `docs/CONFIGURATION.md` lists every key the user config accepts. These tests
//! keep it complete: a new config key fails here until it is documented, and a
//! documented key that `ConfigOverlay::set` no longer knows fails too.

use workspace_engine::{
    ConfigOverlay, HookConfigOverlay, HookEvent, LANGUAGE_SERVER_LANGUAGES, McpServerConfigOverlay,
    ModelProviderConfigOverlay, overlay_field_kinds,
};

fn reference() -> String {
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/CONFIGURATION.md");
    std::fs::read_to_string(path).expect("docs/CONFIGURATION.md is readable")
}

fn documented(reference: &str, key: &str) -> bool {
    reference.contains(&format!("`{key}`"))
}

/// The config key, or key pattern, an overlay field is written as. Most fields
/// are their own key; these are the ones `ConfigOverlay::set` spells
/// differently.
fn key_for_overlay_field(field: &str) -> String {
    if let Some(category) = field.strip_prefix("context_budget_") {
        return format!("context_budget.{category}");
    }
    match field {
        "command_allowlist_by_repository" => "command_allowlist.<repository_id>",
        "web_domain_allowlist_by_repository" => "web_domain_allowlist.<repository_id>",
        "permission_profile_by_repository" => "permission_profile.<repository_id>",
        "hook_reviews_by_repository" => "hook_review.<repository_id>.<hook_id>",
        "language_servers" => "language_server.<language>",
        // The three definition families are checked field by field below.
        "model_providers" => "model_provider.<id>.",
        "mcp_servers" => "mcp_server.<id>.",
        "hooks" => "hook.<id>.",
        other => other,
    }
    .to_string()
}

/// The field names of a per-id overlay, read from its `Debug` output. These
/// structs have no field list of their own, and every field of a default value
/// prints as `None` or an empty string, so the names are what is left.
fn debug_field_names(debug: &str) -> Vec<String> {
    let body = debug
        .split_once('{')
        .and_then(|(_, rest)| rest.rsplit_once('}'))
        .map(|(body, _)| body)
        .expect("a struct's Debug output has braces");
    body.split(", ")
        .filter_map(|field| field.split_once(':').map(|(name, _)| name.trim()))
        .filter(|name| *name != "id")
        .map(str::to_string)
        .collect()
}

/// A concrete key for a documented pattern, so `ConfigOverlay::set` can say
/// whether it knows it.
fn concrete_key(pattern: &str) -> String {
    pattern
        .replace("<repository_id>", "repo_example")
        .replace("<hook_id>", "example")
        .replace("<language>", "rust")
        .replace("<category>", "pinned")
        .replace("<id>", "example")
}

#[test]
fn every_overlay_field_is_documented_under_its_key() {
    let reference = reference();
    let missing: Vec<String> = overlay_field_kinds()
        .into_iter()
        .map(|(field, _)| key_for_overlay_field(field))
        .filter(|key| {
            // A family prefix ends in `.` and is documented as `prefix<field>`.
            if key.ends_with('.') {
                !reference.contains(&format!("`{key}"))
            } else {
                !documented(&reference, key)
            }
        })
        .collect();
    assert!(
        missing.is_empty(),
        "docs/CONFIGURATION.md does not document these config keys: {missing:?}"
    );
}

#[test]
fn every_flat_key_mapping_is_a_key_the_parser_accepts() {
    // Guards the mapping above: a field whose key is spelled differently from
    // its name would otherwise be checked under a name no user can write.
    for (field, _) in overlay_field_kinds() {
        let key = key_for_overlay_field(field);
        if key.ends_with('.') {
            continue;
        }
        let key = concrete_key(&key);
        if let Err(error) = ConfigOverlay::default().set(&key, "x") {
            let message = error.to_string();
            assert!(
                !message.contains("Unknown"),
                "overlay field {field} maps to {key}, which the parser does not know: {message}"
            );
        }
    }
}

#[test]
fn every_definition_field_is_documented() {
    let reference = reference();
    let families = [
        (
            "model_provider.<id>.",
            format!("{:?}", ModelProviderConfigOverlay::default()),
        ),
        (
            "mcp_server.<id>.",
            format!("{:?}", McpServerConfigOverlay::default()),
        ),
        ("hook.<id>.", format!("{:?}", HookConfigOverlay::default())),
    ];
    let mut missing = Vec::new();
    for (prefix, debug) in families {
        let fields = debug_field_names(&debug);
        assert!(!fields.is_empty(), "no fields read for {prefix}");
        for field in fields {
            let key = format!("{prefix}{field}");
            if !documented(&reference, &key) {
                missing.push(key);
            }
        }
    }
    assert!(
        missing.is_empty(),
        "docs/CONFIGURATION.md does not document these fields: {missing:?}"
    );
}

#[test]
fn every_language_and_hook_event_is_listed() {
    let reference = reference();
    for language in LANGUAGE_SERVER_LANGUAGES {
        assert!(
            documented(&reference, language),
            "language_server language {language} is not listed"
        );
    }
    for event in HookEvent::ALL {
        assert!(
            documented(&reference, event.as_str()),
            "hook event {} is not listed",
            event.as_str()
        );
    }
}

#[test]
fn every_documented_key_is_one_the_parser_accepts() {
    // The reverse direction: a key the reference lists in a table's first
    // column must still exist, so a removed or renamed key cannot linger.
    let reference = reference();
    let keys: Vec<&str> = reference
        .lines()
        .filter_map(|line| line.strip_prefix("| `"))
        .filter_map(|rest| rest.split_once('`').map(|(key, _)| key))
        .filter(|key| key.contains('_') || key.contains('.'))
        .collect();
    assert!(keys.len() > 80, "found only {} keys", keys.len());
    for key in keys {
        let concrete = concrete_key(key);
        if let Err(error) = ConfigOverlay::default().set(&concrete, "x") {
            let message = error.to_string();
            assert!(
                !message.contains("Unknown"),
                "docs/CONFIGURATION.md lists {key}, which the parser does not know: {message}"
            );
        }
    }
}
