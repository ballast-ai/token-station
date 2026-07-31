//! The management surface behind `upstream` / `config` / `rule`: every edit is
//! a pure change to [`ClientConfig`] that the caller persists with
//! [`ClientConfig::save`]. The configuration file stays the only source of
//! truth — there is no daemon socket, no second store, and nothing in this
//! module touches the network.
//!
//! A running `serve` does not watch the file; edits apply on restart.

use std::fmt::Write as _;
use std::fs;
use std::path::{Path, PathBuf};

use token_station_protocol::{CapabilityState, ModelCapability, ProviderEndpoint};
use token_station_router_core::{Router, UpstreamModel, UpstreamRef};

use crate::config::{AccessTier, AuthConfig, ClientConfig, UpstreamConfig};

/// Everything `upstream add` collects from the command line, still as text:
/// parsing and refusal both live in [`upstream_add`], so the tests bite the
/// same surface the operator types at.
pub struct AddUpstream<'a> {
    pub name: &'a str,
    pub provider: &'a str,
    pub base_url: &'a str,
    /// `<model>[,tool][,vision][,json-schema][,ctx=N]`, one per `--model`.
    pub models: &'a [String],
    /// `keyring` | `env:<VAR>` | `file:<PATH>`; absent for open upstreams.
    pub auth: Option<&'a str>,
    /// The slot name the provider adapter resolves, e.g. `provider_api_key`.
    pub slot: &'a str,
    /// Append the added models to this pool so routing can reach them.
    pub pool: Option<&'a str>,
    /// This upstream runs on the local machine; a `local_only` route keeps
    /// traffic on it.
    pub local: bool,
}

/// Adds an upstream to the configuration; the caller saves.
///
/// # Errors
///
/// A message for the operator: duplicate name, malformed base URL or model
/// spec, unknown auth source. Cross-checks the file-level validation also
/// covers (a provider with no plugin mapping, a credential pasted into the
/// URL) are refused by [`ClientConfig::save`] before anything reaches disk.
pub fn upstream_add(config: &mut ClientConfig, spec: &AddUpstream) -> Result<String, String> {
    let reference = UpstreamRef::new(spec.name.to_owned()).map_err(|error| error.to_string())?;
    if config.upstreams.contains_key(spec.name) {
        return Err(format!(
            "upstream `{}` already exists; `upstream remove` it first",
            spec.name
        ));
    }

    let base_url = ProviderEndpoint::try_new(spec.base_url)
        .map_err(|error| format!("base url `{}`: {error}", spec.base_url))?;

    if spec.models.is_empty() {
        return Err("an upstream needs at least one --model".to_owned());
    }
    let models = spec
        .models
        .iter()
        .map(|raw| parse_model_spec(raw))
        .collect::<Result<Vec<_>, _>>()?;

    let auth = spec
        .auth
        .map(|source| parse_auth_spec(source, spec.slot))
        .transpose()?;
    let store_hint = matches!(&auth, Some(auth) if auth.store);

    let mut summary = format!(
        "added upstream `{}` ({}, {} model(s))",
        spec.name,
        spec.provider,
        models.len()
    );
    if let Some(pool) = spec.pool {
        let members = config.router.pools.entry(pool.to_owned()).or_default();
        for capability in &models {
            members.push(UpstreamModel::new(
                reference.clone(),
                capability.model.clone(),
            ));
        }
        let _ = write!(summary, "; appended to pool `{pool}`");
    } else {
        let _ = write!(
            summary,
            "; not in any pool yet — no route reaches it until one names it"
        );
    }
    if store_hint {
        let _ = write!(
            summary,
            "\nstore the credential with: token-station-cli key set {} {}",
            spec.name, spec.slot
        );
    }

    config.upstreams.insert(
        spec.name.to_owned(),
        UpstreamConfig {
            provider: spec.provider.to_owned(),
            base_url,
            auth,
            local: spec.local,
            access_tier: AccessTier::default(),
            // `upstream add` creates a Canonical-IR (translated) upstream;
            // anthropic-native passthrough is opted into by editing the config.
            api_dialect: crate::config::ApiDialect::default(),
            models,
            // Quota plans are declared by the desktop app's quota-mode picker,
            // not by `upstream add`.
            quota_plan: None,
        },
    );

    Ok(summary)
}

/// Removes an upstream; refused while any pool still routes to it, because a
/// silently cascading removal could empty a pool a rule depends on.
///
/// # Errors
///
/// Names the unknown upstream, or every pool that still references it.
pub fn upstream_remove(config: &mut ClientConfig, name: &str) -> Result<String, String> {
    let Some(entry) = config.upstreams.get(name) else {
        return Err(format!(
            "no upstream `{name}`; configured: {}",
            config
                .upstreams
                .keys()
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        ));
    };

    let referencing: Vec<&str> = config
        .router
        .pools
        .iter()
        .filter(|(_, members)| {
            members
                .iter()
                .any(|member| member.upstream.as_str() == name)
        })
        .map(|(pool, _)| pool.as_str())
        .collect();
    if !referencing.is_empty() {
        return Err(format!(
            "upstream `{name}` is still routed to by pool(s) {}; edit the routing table first \
             (`config edit`)",
            referencing.join(", ")
        ));
    }

    let mut summary = format!("removed upstream `{name}`");
    if let Some(auth) = &entry.auth {
        if auth.store {
            let _ = write!(
                summary,
                "\nits stored credential survives; delete it with: token-station-cli key remove \
                 {name} {}",
                auth.slot
            );
        }
    }
    config.upstreams.remove(name);
    Ok(summary)
}

/// Renders the configured upstreams as an aligned table.
#[must_use]
pub fn upstream_list(config: &ClientConfig) -> String {
    let mut rows: Vec<[String; 5]> = vec![[
        "NAME".to_owned(),
        "PROVIDER".to_owned(),
        "BASE URL".to_owned(),
        "AUTH".to_owned(),
        "MODELS".to_owned(),
    ]];
    for (name, entry) in &config.upstreams {
        let auth = entry.auth.as_ref().map_or("none".to_owned(), |auth| {
            if auth.store {
                format!("store:{}", auth.slot)
            } else if let Some(variable) = &auth.env {
                format!("env:{variable}")
            } else if let Some(path) = &auth.file {
                format!("file:{}", path.display())
            } else {
                // Refused by config validation before this renders.
                "?".to_owned()
            }
        });
        let models = entry
            .models
            .iter()
            .map(|capability| capability.model.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        rows.push([
            name.clone(),
            entry.provider.clone(),
            serde_json::to_value(&entry.base_url)
                .ok()
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default(),
            auth,
            models,
        ]);
    }
    render_table(&rows)
}

/// Flips one of the closed set of switches. Not a general JSON-path editor:
/// everything else is `config edit`'s job, behind full validation.
///
/// # Errors
///
/// Names the unknown switch and lists the known ones.
pub fn set_switch(config: &mut ClientConfig, switch: &str, value: bool) -> Result<String, String> {
    let state = if value { "on" } else { "off" };
    match switch {
        "server.auth" => {
            config.server.auth = value;
            let mut summary = format!("server.auth = {state}");
            if !value {
                summary.push_str("\nwarning: any local process can use this proxy");
            }
            Ok(summary)
        }
        "data.metrics" => {
            config.data.metrics = value;
            Ok(format!(
                "data.metrics = {state} (the file log is always written)"
            ))
        }
        other => Err(format!(
            "unknown switch `{other}`; known switches: server.auth, data.metrics"
        )),
    }
}

/// Renders the routing table, layer by layer, in evaluation order.
///
/// Matchers print as the JSON they are in the file — a rendering DSL would be
/// a second syntax to keep in lockstep with `router-core`'s schema.
#[must_use]
pub fn rule_list(config: &ClientConfig) -> String {
    let router = &config.router;
    let mut out = String::from("routing table (first layer to decide wins):\n");

    out.push_str("layer 1 — rules, in order:\n");
    if router.rules.is_empty() {
        out.push_str("  (none)\n");
    }
    for (index, rule) in router.rules.iter().enumerate() {
        let matcher = serde_json::to_string(&rule.matcher).unwrap_or_default();
        let _ = writeln!(
            out,
            "  {}. {} when {matcher} -> {}",
            index + 1,
            rule.id,
            rule.route_to
        );
    }

    out.push_str("layer 2 — hint routes:\n");
    if router.hint_routes.is_empty() {
        out.push_str("  (none)\n");
    }
    for hint in &router.hint_routes {
        let kind = serde_json::to_string(&hint.kind).unwrap_or_default();
        let _ = writeln!(
            out,
            "  {}={} -> {}",
            kind.trim_matches('"'),
            hint.value,
            hint.route_to
        );
    }

    match &router.heuristic {
        Some(heuristic) => {
            let _ = writeln!(
                out,
                "layer 3 — heuristic: score >= {} -> {}, else {}",
                heuristic.threshold, heuristic.above, heuristic.below
            );
        }
        None => out.push_str("layer 3 — heuristic: (none)\n"),
    }

    let _ = writeln!(out, "default pool: {}", router.default_pool);

    out.push_str("pools:\n");
    for (pool, members) in &router.pools {
        let members = members
            .iter()
            .map(|member| format!("{}/{}", member.upstream, member.model))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "  {pool}: {members}");
    }
    out
}

/// `config edit`: the whole file in the operator's editor, applied only if the
/// result passes everything `serve` would check at startup — including the
/// routing table's own coherence, so an unroutable edit fails here and not at
/// the next start.
///
/// The editor works on a draft copy. An invalid draft is kept, and the next
/// `config edit` resumes from it; the real file changes atomically or not at
/// all.
///
/// # Errors
///
/// The editor failing to launch, or the draft failing validation — with the
/// draft's path, so no work is lost.
pub fn edit(config_path: &Path, editor: &str) -> Result<String, String> {
    let draft = draft_path(config_path)?;
    let resumed = draft.exists();
    if !resumed {
        fs::copy(config_path, &draft).map_err(|error| {
            format!(
                "cannot copy `{}` to a draft: {error}",
                config_path.display()
            )
        })?;
    }

    let mut words = editor.split_whitespace();
    let program = words
        .next()
        .ok_or_else(|| "the editor command is empty".to_owned())?;
    let status = std::process::Command::new(program)
        .args(words)
        .arg(&draft)
        .status()
        .map_err(|error| format!("cannot launch editor `{editor}`: {error}"))?;
    if !status.success() {
        return Err(format!(
            "editor exited with {status}; draft kept at `{}`",
            draft.display()
        ));
    }

    let config =
        ClientConfig::load(&draft).map_err(|error| keep_draft(&draft, &error.to_string()))?;
    Router::new(config.router.clone())
        .map_err(|error| keep_draft(&draft, &format!("routing table: {error}")))?;

    config
        .save(config_path)
        .map_err(|error| error.to_string())?;
    fs::remove_file(&draft).ok();

    Ok(if resumed {
        "configuration updated (from the kept draft)".to_owned()
    } else {
        "configuration updated".to_owned()
    })
}

fn keep_draft(draft: &Path, detail: &str) -> String {
    format!(
        "{detail}\nnothing was applied; the draft is kept at `{}` — run `config edit` again to \
         resume it",
        draft.display()
    )
}

fn draft_path(config_path: &Path) -> Result<PathBuf, String> {
    let file_name = config_path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .ok_or_else(|| format!("`{}` has no file name", config_path.display()))?;
    Ok(config_path.with_file_name(format!("{file_name}.draft")))
}

/// `<model>[,tool][,vision][,json-schema][,ctx=N]` — a closed vocabulary, so a
/// typo is a refusal that lists it, not a capability that silently reads as
/// unsupported.
fn parse_model_spec(raw: &str) -> Result<ModelCapability, String> {
    let mut tokens = raw.split(',').map(str::trim);
    let model = tokens
        .next()
        .filter(|token| !token.is_empty())
        .ok_or_else(|| format!("model spec `{raw}` names no model"))?;

    let mut capability = ModelCapability {
        model: model.to_owned(),
        ..ModelCapability::default()
    };
    for token in tokens {
        match token {
            "tool" => {
                capability.tool = true;
                capability.tool_state = Some(CapabilityState::Declared);
            }
            "vision" => {
                capability.vision = true;
                capability.vision_state = Some(CapabilityState::Declared);
            }
            "json-schema" => {
                capability.json_schema = true;
                capability.json_schema_state = Some(CapabilityState::Declared);
            }
            _ => {
                if let Some(window) = token.strip_prefix("ctx=") {
                    capability.context_window = window.parse().map_err(|_| {
                        format!("model spec `{raw}`: `{token}` is not `ctx=<tokens>`")
                    })?;
                } else {
                    return Err(format!(
                        "model spec `{raw}`: unknown capability `{token}`; known: tool, vision, \
                         json-schema, ctx=<tokens>"
                    ));
                }
            }
        }
    }
    Ok(capability)
}

/// `store` | `env:<VAR>` | `file:<PATH>` — where the credential *lives*;
/// the value itself never appears on a command line.
fn parse_auth_spec(source: &str, slot: &str) -> Result<AuthConfig, String> {
    let mut auth = AuthConfig {
        slot: slot.to_owned(),
        store: false,
        env: None,
        file: None,
    };
    if source == "store" || source == "keyring" {
        auth.store = true;
    } else if let Some(variable) = source.strip_prefix("env:") {
        auth.env = Some(variable.to_owned());
    } else if let Some(path) = source.strip_prefix("file:") {
        auth.file = Some(PathBuf::from(path));
    } else {
        return Err(format!(
            "auth source `{source}` is not one of: keyring, env:<VAR>, file:<PATH>"
        ));
    }
    Ok(auth)
}

fn render_table(rows: &[[String; 5]]) -> String {
    let mut widths = [0usize; 5];
    for row in rows {
        for (column, cell) in row.iter().enumerate() {
            widths[column] = widths[column].max(cell.chars().count());
        }
    }
    let mut out = String::new();
    for row in rows {
        for (column, cell) in row.iter().enumerate() {
            if column + 1 == row.len() {
                let _ = writeln!(out, "{cell}");
            } else {
                let _ = write!(out, "{cell:width$}  ", width = widths[column]);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{AddUpstream, upstream_add, upstream_remove};
    use crate::config::ClientConfig;

    fn example() -> ClientConfig {
        serde_json::from_str(crate::EXAMPLE_CONFIG).expect("the shipped example parses")
    }

    fn add_spec<'a>(name: &'a str, models: &'a [String]) -> AddUpstream<'a> {
        AddUpstream {
            name,
            provider: "openai-compatible",
            base_url: "https://api.example.com/v1",
            models,
            auth: None,
            slot: "provider_api_key",
            pool: None,
            local: false,
        }
    }

    #[test]
    fn a_local_upstream_is_flagged_for_local_only_routing() {
        let mut config = example();
        let models = vec!["qwen3,tool".to_owned()];
        let spec = AddUpstream {
            base_url: "http://127.0.0.1:1234/v1",
            local: true,
            ..add_spec("lmstudio_local", &models)
        };

        upstream_add(&mut config, &spec).expect("adds a local upstream");
        assert!(
            config.upstreams["lmstudio_local"].local,
            "the upstream is flagged local so local_only routing can keep to it"
        );
        // The flag survives a save/load round-trip like any other config field.
        let path =
            std::env::temp_dir().join(format!("ts-manage-local-{}.json", std::process::id()));
        config.save(&path).expect("saves");
        let reloaded = ClientConfig::load(&path).expect("reloads");
        assert!(reloaded.upstreams["lmstudio_local"].local);
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn an_added_upstream_lands_in_config_and_its_pool() {
        let mut config = example();
        let models = vec!["m1,tool,ctx=8192".to_owned()];
        let spec = AddUpstream {
            pool: Some("cheap"),
            auth: Some("env:EXAMPLE_KEY"),
            ..add_spec("example_new", &models)
        };

        let summary = upstream_add(&mut config, &spec).expect("adds");
        assert!(summary.contains("pool `cheap`"), "{summary}");

        let added = &config.upstreams["example_new"];
        assert_eq!(added.models[0].model, "m1");
        assert!(added.models[0].tool_state().is_supported());
        assert_eq!(added.models[0].context_window, 8192);
        assert_eq!(
            added.auth.as_ref().expect("auth").env.as_deref(),
            Some("EXAMPLE_KEY")
        );
        assert!(
            config.router.pools["cheap"]
                .iter()
                .any(|member| member.upstream.as_str() == "example_new")
        );
        // The whole edited config still passes the same validation `load` runs.
        let path = std::env::temp_dir().join(format!("ts-manage-add-{}.json", std::process::id()));
        config.save(&path).expect("saves");
        ClientConfig::load(&path).expect("reloads");
        std::fs::remove_file(path).ok();
    }

    #[test]
    fn adding_a_duplicate_upstream_is_refused() {
        let mut config = example();
        let models = vec!["m1".to_owned()];

        let error = upstream_add(&mut config, &add_spec("openai_personal", &models))
            .expect_err("duplicate");
        assert!(error.contains("already exists"), "{error}");
        assert_eq!(config.upstreams.len(), 2, "nothing was added");
    }

    #[test]
    fn a_key_pasted_into_the_base_url_is_refused() {
        let mut config = example();
        let models = vec!["m1".to_owned()];
        let spec = AddUpstream {
            base_url: "https://api.example.com/v1?api-key=sk-live-abc",
            ..add_spec("example_new", &models)
        };

        let error = upstream_add(&mut config, &spec).expect_err("query strings carry keys");
        assert!(error.contains("base url"), "{error}");
    }

    #[test]
    fn a_typoed_capability_is_refused_with_the_vocabulary() {
        let mut config = example();
        let models = vec!["m1,tools".to_owned()];

        let error = upstream_add(&mut config, &add_spec("example_new", &models))
            .expect_err("`tools` is not `tool`");
        assert!(error.contains("unknown capability `tools`"), "{error}");
        assert!(error.contains("json-schema"), "{error}");
    }

    #[test]
    fn a_garbled_auth_source_is_refused() {
        let mut config = example();
        let models = vec!["m1".to_owned()];
        let spec = AddUpstream {
            auth: Some("keychain"),
            ..add_spec("example_new", &models)
        };

        let error = upstream_add(&mut config, &spec).expect_err("`keychain` is not a source");
        assert!(error.contains("keyring, env:<VAR>, file:<PATH>"), "{error}");
    }

    #[test]
    fn removing_a_pool_member_is_refused_naming_the_pool() {
        let mut config = example();

        let error = upstream_remove(&mut config, "openai_personal").expect_err("sota routes here");
        assert!(error.contains("sota"), "{error}");
        assert!(config.upstreams.contains_key("openai_personal"));
    }

    #[test]
    fn removing_an_unreferenced_upstream_succeeds_with_a_keychain_hint() {
        let mut config = example();
        let models = vec!["m1".to_owned()];
        let spec = AddUpstream {
            auth: Some("keyring"),
            ..add_spec("example_new", &models)
        };
        upstream_add(&mut config, &spec).expect("adds");

        let summary = upstream_remove(&mut config, "example_new").expect("removes");
        assert!(!config.upstreams.contains_key("example_new"));
        assert!(
            summary.contains("key remove example_new provider_api_key"),
            "{summary}"
        );
    }

    #[test]
    fn removing_an_unknown_upstream_lists_what_exists() {
        let mut config = example();

        let error = upstream_remove(&mut config, "nowhere").expect_err("no such upstream");
        assert!(error.contains("ollama_local"), "{error}");
    }

    #[test]
    fn switches_flip_and_unknown_switches_list_the_known_ones() {
        let mut config = example();
        assert!(config.server.auth, "auth defaults on");

        let summary = super::set_switch(&mut config, "server.auth", false).expect("flips");
        assert!(!config.server.auth);
        assert!(summary.contains("warning"), "{summary}");

        super::set_switch(&mut config, "data.metrics", false).expect("flips");
        assert!(!config.data.metrics);

        let error =
            super::set_switch(&mut config, "server.listen", true).expect_err("not a switch");
        assert!(error.contains("server.auth, data.metrics"), "{error}");
    }

    #[test]
    fn the_rule_listing_walks_the_layers_in_evaluation_order() {
        let config = example();
        let listing = super::rule_list(&config);

        let rule_one = listing.find("1. long-context").expect("first rule");
        let rule_two = listing.find("2. tool-calls").expect("second rule");
        let hints = listing.find("step_type=planning -> sota").expect("hint");
        let heuristic = listing.find("score >= 40").expect("heuristic");
        let default = listing.find("default pool: cheap").expect("default");
        assert!(
            rule_one < rule_two && rule_two < hints && hints < heuristic && heuristic < default
        );
    }

    #[test]
    fn the_upstream_listing_names_the_auth_source_but_never_a_value() {
        let config = example();
        let listing = super::upstream_list(&config);

        assert!(listing.contains("env:OPENAI_API_KEY"), "{listing}");
        assert!(listing.contains("none"), "ollama has no auth: {listing}");
        assert!(listing.contains("https://api.openai.com/v1"), "{listing}");
    }

    #[cfg(unix)]
    mod editing {
        use crate::config::ClientConfig;
        use std::fs;
        use std::path::PathBuf;

        /// A scratch config file plus an "editor": `sh <script>` that ignores
        /// the draft it is handed and overwrites it with `replacement`.
        fn scene(name: &str, replacement: &str) -> (PathBuf, String) {
            let dir =
                std::env::temp_dir().join(format!("ts-manage-edit-{}-{name}", std::process::id()));
            fs::create_dir_all(&dir).expect("temp dir");
            let config_path = dir.join("token-station.json");
            fs::write(&config_path, crate::EXAMPLE_CONFIG).expect("writes");

            let replacement_path = dir.join("replacement.json");
            fs::write(&replacement_path, replacement).expect("writes");
            let script = dir.join("editor.sh");
            fs::write(
                &script,
                format!("cp '{}' \"$1\"\n", replacement_path.display()),
            )
            .expect("writes");

            (config_path, format!("sh {}", script.display()))
        }

        #[test]
        fn a_valid_edit_replaces_the_config_and_clears_the_draft() {
            let mut edited: serde_json::Value =
                serde_json::from_str(crate::EXAMPLE_CONFIG).expect("parses");
            edited["router"]["default_pool"] = serde_json::json!("sota");
            let (config_path, editor) = scene("valid", &edited.to_string());

            super::super::edit(&config_path, &editor).expect("applies");

            let reloaded = ClientConfig::load(&config_path).expect("reloads");
            assert_eq!(reloaded.router.default_pool, "sota");
            assert!(
                !config_path
                    .with_file_name("token-station.json.draft")
                    .exists()
            );
        }

        #[test]
        fn an_invalid_edit_keeps_the_draft_and_touches_nothing() {
            let mut broken: serde_json::Value =
                serde_json::from_str(crate::EXAMPLE_CONFIG).expect("parses");
            broken["router"]["default_pool"] = serde_json::json!("no-such-pool");
            let (config_path, editor) = scene("invalid", &broken.to_string());
            let before = fs::read_to_string(&config_path).expect("reads");

            let error = super::super::edit(&config_path, &editor).expect_err("unroutable");
            assert!(error.contains("draft is kept"), "{error}");

            assert_eq!(
                fs::read_to_string(&config_path).expect("reads"),
                before,
                "the real file must be untouched"
            );
            let draft = config_path.with_file_name("token-station.json.draft");
            assert!(draft.exists(), "the operator's work is not thrown away");

            // Round two resumes the draft: a fixing editor now succeeds.
            let mut fixed: serde_json::Value =
                serde_json::from_str(crate::EXAMPLE_CONFIG).expect("parses");
            fixed["router"]["default_pool"] = serde_json::json!("sota");
            let replacement = draft.with_file_name("fixed.json");
            fs::write(&replacement, fixed.to_string()).expect("writes");
            let script = draft.with_file_name("fix-editor.sh");
            fs::write(&script, format!("cp '{}' \"$1\"\n", replacement.display())).expect("writes");

            let summary = super::super::edit(&config_path, &format!("sh {}", script.display()))
                .expect("the fixed draft applies");
            assert!(summary.contains("kept draft"), "{summary}");
            assert!(!draft.exists());
            let reloaded = ClientConfig::load(&config_path).expect("reloads");
            assert_eq!(reloaded.router.default_pool, "sota");
        }

        #[test]
        fn an_editor_that_exits_nonzero_applies_nothing() {
            let (config_path, _) = scene("abort", "{}");
            let before = fs::read_to_string(&config_path).expect("reads");
            let script = config_path.with_file_name("abort-editor.sh");
            fs::write(&script, "exit 1\n").expect("writes");

            let error = super::super::edit(&config_path, &format!("sh {}", script.display()))
                .expect_err("editor aborted");
            assert!(error.contains("draft kept"), "{error}");
            assert_eq!(fs::read_to_string(&config_path).expect("reads"), before);
        }
    }
}
