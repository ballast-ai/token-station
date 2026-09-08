//! Offering-scoped synchronization. Remote data never owns manual price rows.

use crate::*;
use serde::{Deserialize, Serialize};

const SYNC_INTERVAL_MS: u64 = 6 * 60 * 60 * 1_000;
const RETRY_INTERVAL_MS: u64 = 15 * 60 * 1_000;
const STATE_FILE: &str = "price-sync.json";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
struct SyncMetadata {
    enabled: bool,
    last_attempt_ms: Option<u64>,
    last_sync_ms: Option<u64>,
    last_inventory: String,
    errors: Vec<String>,
    records: BTreeMap<String, SyncedPrice>,
    suppressed: BTreeSet<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct SyncedPrice {
    price: ModelPrice,
    source: String,
    identity: String,
    fetched_at_ms: u64,
}

fn merge_prices(
    current: &PriceTable,
    metadata: &mut SyncMetadata,
    incoming: BTreeMap<String, SyncedPrice>,
) -> Result<PriceTable, String> {
    let mut changes = BTreeMap::new();
    let mut records = metadata.records.clone();
    for (key, incoming) in incoming {
        if metadata.suppressed.contains(&key) {
            continue;
        }
        if let Some(existing) = current.models.get(&key) {
            let owned = records.get(&key).is_some_and(|record| {
                record.price == *existing
                    && record.identity == incoming.identity
                    && !(record.source == "provider" && incoming.source != "provider")
            });
            if !owned {
                continue;
            }
        }
        incoming.price.validate()?;
        if current.models.get(&key) != Some(&incoming.price) {
            changes.insert(key.clone(), incoming.price);
        }
        records.insert(key, incoming);
    }
    let next = if changes.is_empty() {
        current.clone()
    } else {
        current.next_with_models(changes)?
    };
    metadata.records = records;
    Ok(next)
}

#[derive(Serialize)]
pub(crate) struct PriceOfferingView {
    upstream: String,
    model: String,
    key: String,
    price: Option<ModelPrice>,
    source: String,
    fetched_at_ms: Option<u64>,
}

#[derive(Serialize)]
pub(crate) struct PriceSyncView {
    enabled: bool,
    running: bool,
    last_attempt_ms: Option<u64>,
    last_sync_ms: Option<u64>,
    errors: Vec<String>,
}

#[derive(Serialize)]
pub(crate) struct PricingInventoryView {
    table: PriceTable,
    offerings: Vec<PriceOfferingView>,
    sync: PriceSyncView,
    requires_apply: bool,
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u64::MAX as u128) as u64
}

fn fingerprint(value: &Value) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(value.to_string().as_bytes()))
}

fn provider_identity(upstream: &Value) -> String {
    let mut identity = upstream.clone();
    if let Some(object) = identity.as_object_mut() {
        object.remove("models");
    }
    fingerprint(&identity)
}

fn read_metadata(data_dir: &Path) -> Result<SyncMetadata, String> {
    match std::fs::read(data_dir.join(STATE_FILE)) {
        Ok(bytes) if bytes.len() <= 8 * 1024 * 1024 => {
            serde_json::from_slice(&bytes).map_err(|_| {
                "The price synchronization state is invalid. Existing prices are unchanged."
                    .to_owned()
            })
        }
        Ok(_) => Err("The price synchronization state exceeds its size limit.".to_owned()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(SyncMetadata::default()),
        Err(_) => Err("Cannot read the price synchronization state.".to_owned()),
    }
}

fn write_metadata(data_dir: &Path, metadata: &SyncMetadata) -> Result<(), String> {
    let bytes = serde_json::to_vec_pretty(metadata).map_err(|error| error.to_string())?;
    crate::agent_integration::safe_fs::write_atomic_private(&data_dir.join(STATE_FILE), &bytes)
        .map_err(|_| "Cannot save the price synchronization state.".to_owned())
}

pub(crate) fn mark_manual(
    inner: &mut AppInner,
    key: &str,
    deleted: bool,
    commit: impl FnOnce(&mut AppInner) -> Result<(), String>,
) -> Result<(), String> {
    let data_dir = inner.data_dir();
    let previous = read_metadata(&data_dir)?;
    let mut metadata = previous.clone();
    metadata.records.remove(key);
    if deleted {
        metadata.suppressed.insert(key.to_owned());
    } else {
        metadata.suppressed.remove(key);
    }
    // Keep the App lock across both writes. A failed configuration edit must not
    // turn a synchronized row into a manual row or leave a deletion tombstone.
    let result = write_metadata(&data_dir, &metadata).and_then(|_| commit(inner));
    match result {
        Ok(()) => Ok(()),
        Err(error) => match write_metadata(&data_dir, &previous) {
            Ok(()) => Err(error),
            Err(rollback_error) => Err(format!(
                "{error} Price synchronization ownership rollback also failed: {rollback_error}"
            )),
        },
    }
}

fn offering_views(
    draft: &Value,
    table: &PriceTable,
    metadata: &SyncMetadata,
) -> Vec<PriceOfferingView> {
    let mut offerings = Vec::new();
    if let Some(upstreams) = draft["upstreams"].as_object() {
        for (name, upstream) in upstreams {
            let identity = provider_identity(upstream);
            let models: BTreeSet<&str> = upstream["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|value| value["model"].as_str())
                .collect();
            for model in models {
                let key = format!("{name}/{model}");
                let exact = table.models.get(&key);
                let price = exact.or_else(|| table.model_price_for_upstream(name, model));
                let record = metadata
                    .records
                    .get(&key)
                    .filter(|record| exact == Some(&record.price) && record.identity == identity);
                let source = if let Some(record) = record {
                    record.source.as_str()
                } else if exact.is_some() {
                    "manual"
                } else if price.is_some() {
                    "fallback"
                } else {
                    "missing"
                };
                offerings.push(PriceOfferingView {
                    upstream: name.clone(),
                    model: model.to_owned(),
                    key,
                    price: price.copied(),
                    source: source.to_owned(),
                    fetched_at_ms: record.map(|record| record.fetched_at_ms),
                });
            }
        }
    }
    offerings
}

fn inventory(inner: &AppInner) -> Result<PricingInventoryView, String> {
    let table = draft_price_table(inner)?;
    let metadata = read_metadata(&inner.data_dir())?;
    let offerings = offering_views(&inner.draft, &table, &metadata);
    Ok(PricingInventoryView {
        table,
        offerings,
        requires_apply: inner
            .serve_view()
            .running_revision
            .is_some_and(|revision| revision != inner.config_state.saved_revision()),
        sync: PriceSyncView {
            enabled: metadata.enabled,
            running: inner.price_sync_running,
            last_attempt_ms: metadata.last_attempt_ms,
            last_sync_ms: metadata.last_sync_ms,
            errors: metadata.errors,
        },
    })
}

#[tauri::command]
pub(crate) fn get_pricing_inventory(
    state: State<'_, AppStateManaged>,
) -> Result<PricingInventoryView, String> {
    inventory(&state.0.lock().unwrap())
}

#[tauri::command]
pub(crate) fn set_price_sync_enabled(
    state: State<'_, AppStateManaged>,
    enabled: bool,
) -> Result<PricingInventoryView, String> {
    let inner = state.0.lock().unwrap();
    inner.ensure_editable()?;
    let mut metadata = read_metadata(&inner.data_dir())?;
    metadata.enabled = enabled;
    // An explicit enable requests an immediate new check, including after a failure.
    if enabled {
        metadata.last_attempt_ms = None;
        metadata.last_sync_ms = None;
    }
    write_metadata(&inner.data_dir(), &metadata)?;
    inventory(&inner)
}

struct SyncGuard<'a>(&'a Mutex<AppInner>);
impl Drop for SyncGuard<'_> {
    fn drop(&mut self) {
        self.0.lock().unwrap().price_sync_running = false;
    }
}

struct FetchTarget {
    name: String,
    base_url: String,
    auth_slot: Option<String>,
    models: Vec<String>,
    public_provider: Option<String>,
    identity: String,
}

struct FetchResult {
    prices: BTreeMap<String, SyncedPrice>,
    errors: Vec<String>,
}

fn save_synced_table(inner: &mut AppInner, next: &PriceTable) -> Result<(), String> {
    let previous_draft = inner.draft.clone();
    inner.draft["pricing"] = serde_json::to_value(next).map_err(|error| error.to_string())?;
    if let Err(error) = inner
        .observe_draft()
        .and_then(|_| inner.save_draft_without_backfill())
    {
        inner.draft = previous_draft;
        // Observe the restored content without rolling back the reserved revision
        // watermark. The failed revision must never identify a later draft.
        if let Err(rollback) = inner.observe_draft() {
            let message = format!("{error}; cannot restore the price draft: {rollback}");
            inner.load_error = Some(message.clone());
            return Err(message);
        }
        return Err(error);
    }
    Ok(())
}

fn fetch_prices(
    data_dir: &Path,
    targets: Vec<FetchTarget>,
    egress: &token_station_cli::config::EgressConfig,
    secrets: &secrets::SecretStore,
) -> FetchResult {
    let mut result = FetchResult {
        prices: BTreeMap::new(),
        errors: Vec::new(),
    };
    for target in targets {
        let mut prices = BTreeMap::new();
        // Public catalogs receive no channel credentials. Channel discovery is restricted
        // to its configured HTTPS endpoint and returns only current model metadata.
        let channel = (|| {
            let key = target
                .auth_slot
                .as_ref()
                .map(|slot| secrets.resolve(&target.name, slot))
                .transpose()?
                .map(Zeroizing::new);
            model_catalog::fetch_prices_live_egress(
                &target.base_url,
                key.as_ref().map(|v| v.as_str()),
                egress,
                secrets,
            )
        })();
        let mut channel_failed = false;
        match channel {
            Ok(catalog) => {
                for model in catalog {
                    if target.models.contains(&model.model) {
                        if let Some(price) =
                            model.cost.as_ref().and_then(catalog_cost_to_model_price)
                        {
                            prices.insert(
                                model.model,
                                SyncedPrice {
                                    price,
                                    source: "provider".to_owned(),
                                    identity: target.identity.clone(),
                                    fetched_at_ms: now_ms(),
                                },
                            );
                        }
                    }
                }
            }
            Err(_) => {
                channel_failed = true;
            }
        }
        let missing: Vec<String> = target
            .models
            .iter()
            .filter(|model| !prices.contains_key(*model))
            .cloned()
            .collect();
        if !missing.is_empty() {
            if let Some(provider) = &target.public_provider {
                match pricing_catalog::suggest_many_live_with_egress(
                    data_dir,
                    Some(provider),
                    &missing,
                    egress,
                    secrets,
                ) {
                    Ok(suggestions) => {
                        for requested in suggestions {
                            let suggestion = requested.suggestion;
                            prices
                                .entry(requested.requested_model_id)
                                .or_insert_with(|| SyncedPrice {
                                    price: ModelPrice {
                                        input_per_mtok: suggestion.input_per_mtok,
                                        output_per_mtok: suggestion.output_per_mtok,
                                        cache_read_per_mtok: suggestion.cache_read_per_mtok,
                                        cache_write_per_mtok: suggestion.cache_write_per_mtok,
                                        reasoning_per_mtok: suggestion.reasoning_per_mtok,
                                        ..Default::default()
                                    },
                                    source: "models.dev".to_owned(),
                                    identity: target.identity.clone(),
                                    fetched_at_ms: suggestion.fetched_at_ms,
                                });
                        }
                    }
                    Err(_) => result.errors.push(format!(
                        "{}: public price catalog unavailable; existing prices kept.",
                        target.name
                    )),
                }
            } else if channel_failed {
                result.errors.push(format!(
                    "{}: channel price catalog unavailable; existing prices kept.",
                    target.name
                ));
            }
        }
        for (model, price) in prices {
            result
                .prices
                .insert(format!("{}/{model}", target.name), price);
        }
    }
    result
}

async fn synchronize(
    state: &AppStateManaged,
    automatic: bool,
) -> Result<PricingInventoryView, String> {
    let (data_dir, config, targets, revision, epochs, expected_enabled, inventory_id) = {
        let mut inner = state.0.lock().unwrap();
        inner.ensure_editable()?;
        if inner.price_sync_running {
            return Err("Price synchronization is already in progress.".to_owned());
        }
        if inner.config_state.is_dirty() || !inner.pending_provider_keys.is_empty() {
            return Err(
                "Save or discard pending configuration changes before synchronizing prices."
                    .to_owned(),
            );
        }
        let mut metadata = read_metadata(&inner.data_dir())?;
        let inventory_id = fingerprint(&inner.draft["upstreams"]);
        if automatic && !due(&metadata, now_ms(), &inventory_id) {
            return inventory(&inner);
        }
        let config = inner.materialize()?;
        let mut targets = Vec::new();
        for (name, upstream) in &config.upstreams {
            if inner.draft["upstreams"][name]["provider"].as_str() != Some("openai-compatible") {
                continue;
            }
            let Some(base_url) = inner.draft["upstreams"][name]["base_url"].as_str() else {
                continue;
            };
            let endpoint =
                ProviderEndpoint::try_new(base_url).map_err(|error| error.to_string())?;
            // Automatic catalog access must not contact local services or forward plaintext credentials.
            if endpoint.is_loopback() || !endpoint.uses_https() {
                continue;
            }
            targets.push(FetchTarget {
                name: name.clone(),
                base_url: endpoint.as_str(),
                auth_slot: upstream.auth.as_ref().map(|auth| auth.slot.clone()),
                models: configured_upstream_models(&inner, name)?
                    .into_iter()
                    .collect(),
                // Subscription catalog zeros describe plan inclusion, not free API usage.
                public_provider: configured_public_price_provider_id(&inner, name)
                    .ok()
                    .filter(|provider| !provider.ends_with("-plan")),
                identity: provider_identity(&inner.draft["upstreams"][name]),
            });
        }
        metadata.last_attempt_ms = Some(now_ms());
        write_metadata(&inner.data_dir(), &metadata)?;
        inner.price_sync_running = true;
        (
            inner.data_dir(),
            config,
            targets,
            inner.config_state.draft_revision(),
            inner.upstream_epochs.clone(),
            metadata.enabled,
            inventory_id,
        )
    };
    let guard = SyncGuard(&state.0);
    let task_dir = data_dir.clone();
    let fetched = tauri::async_runtime::spawn_blocking(move || {
        let secrets = secrets::SecretStore::from_config(&config, &task_dir);
        fetch_prices(&task_dir, targets, &config.egress, &secrets)
    })
    .await
    .map_err(|_| "The price synchronization task failed.".to_owned())?;
    let pricing = {
        let mut inner = state.0.lock().unwrap();
        inner.ensure_editable()?;
        let mut metadata = read_metadata(&data_dir)?;
        if inner.config_state.is_dirty()
            || inner.config_state.draft_revision() != revision
            || inner.upstream_epochs != epochs
            || (automatic && metadata.enabled != expected_enabled)
        {
            return Err(
                "Configuration changed during price synchronization. Retry the synchronization."
                    .to_owned(),
            );
        }
        let current = draft_price_table(&inner)?;
        let next = merge_prices(&current, &mut metadata, fetched.prices)?;
        if next != current {
            save_synced_table(&mut inner, &next)?;
        }
        metadata.errors = fetched.errors;
        metadata.last_inventory = inventory_id;
        if metadata.errors.is_empty() {
            metadata.last_sync_ms = Some(now_ms());
        }
        // If this write fails after a config commit, unowned rows become protected manual
        // rows. A retry cannot overwrite them based on an uncommitted ownership claim.
        write_metadata(&data_dir, &metadata)?;
        next
    };
    let backfill_dir = data_dir.clone();
    let backfill = tauri::async_runtime::spawn_blocking(move || {
        SqliteStore::backfill_unknown_costs(&backfill_dir.join("metrics.sqlite"), &pricing)
    })
    .await
    .map_err(|_| "The historical cost task failed.".to_owned())?;
    if backfill.is_err() {
        let _inner = state.0.lock().unwrap();
        let mut metadata = read_metadata(&data_dir)?;
        metadata.errors.push(
            "Prices saved. Historical cost backfill failed; retry synchronization.".to_owned(),
        );
        write_metadata(&data_dir, &metadata)?;
    }
    drop(guard);
    inventory(&state.0.lock().unwrap())
}

#[tauri::command]
pub(crate) async fn sync_model_prices(
    state: State<'_, AppStateManaged>,
) -> Result<PricingInventoryView, String> {
    synchronize(state.inner(), false).await
}

pub(crate) fn start_scheduler(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        while let Some(state) = app.try_state::<AppStateManaged>() {
            let _ = synchronize(state.inner(), true).await;
            tokio::time::sleep(Duration::from_secs(60)).await;
        }
    });
}

fn due(metadata: &SyncMetadata, now: u64, inventory: &str) -> bool {
    metadata.enabled
        && metadata
            .last_attempt_ms
            .is_none_or(|at| now.saturating_sub(at) >= RETRY_INTERVAL_MS)
        && (metadata.last_inventory != inventory
            || metadata
                .last_sync_ms
                .is_none_or(|at| now.saturating_sub(at) >= SYNC_INTERVAL_MS))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(rate: u64, identity: &str) -> SyncedPrice {
        SyncedPrice {
            price: ModelPrice {
                input_per_mtok: rate,
                output_per_mtok: rate * 2,
                ..Default::default()
            },
            source: "provider".to_owned(),
            identity: identity.to_owned(),
            fetched_at_ms: 100,
        }
    }

    #[test]
    fn synchronization_adds_missing_scoped_prices_in_one_revision() {
        let current = PriceTable::default();
        let mut metadata = SyncMetadata::default();
        let next = merge_prices(
            &current,
            &mut metadata,
            BTreeMap::from([
                ("channel/a".to_owned(), row(12, "one")),
                ("channel/b".to_owned(), row(23, "one")),
            ]),
        )
        .unwrap();
        assert_eq!(next.models.len(), 2);
        assert_eq!(next.version, current.version + 1);
        assert_eq!(metadata.records.len(), 2);
    }

    #[test]
    fn synchronization_updates_owned_prices_but_keeps_manual_and_deleted_rows() {
        let mut metadata = SyncMetadata::default();
        let current = merge_prices(
            &PriceTable::default(),
            &mut metadata,
            BTreeMap::from([("channel/a".to_owned(), row(12, "one"))]),
        )
        .unwrap();
        let current = current
            .next_with_model("channel/manual", row(99, "one").price)
            .unwrap();
        metadata.suppressed.insert("channel/deleted".to_owned());
        let next = merge_prices(
            &current,
            &mut metadata,
            BTreeMap::from([
                ("channel/a".to_owned(), row(20, "one")),
                ("channel/manual".to_owned(), row(1, "one")),
                ("channel/deleted".to_owned(), row(1, "one")),
            ]),
        )
        .unwrap();
        assert_eq!(next.models["channel/a"].input_per_mtok, 20);
        assert_eq!(next.models["channel/manual"].input_per_mtok, 99);
        assert!(!next.models.contains_key("channel/deleted"));
    }

    #[test]
    fn synchronization_does_not_overwrite_edits_or_reuse_other_channel_ownership() {
        let mut metadata = SyncMetadata::default();
        metadata
            .records
            .insert("channel/a".to_owned(), row(12, "one"));
        let current = PriceTable::default()
            .next_with_model("channel/a", row(13, "one").price)
            .unwrap();
        let next = merge_prices(
            &current,
            &mut metadata,
            BTreeMap::from([("channel/a".to_owned(), row(20, "one"))]),
        )
        .unwrap();
        assert_eq!(next, current);
        let current = current
            .next_with_model("channel/a", row(12, "one").price)
            .unwrap();
        let next = merge_prices(
            &current,
            &mut metadata,
            BTreeMap::from([("channel/a".to_owned(), row(20, "two"))]),
        )
        .unwrap();
        assert_eq!(next, current);
    }

    #[test]
    fn empty_or_unchanged_results_keep_prices_and_revision() {
        let mut metadata = SyncMetadata::default();
        let incoming = BTreeMap::from([("channel/a".to_owned(), row(12, "one"))]);
        let current =
            merge_prices(&PriceTable::default(), &mut metadata, incoming.clone()).unwrap();
        assert_eq!(
            merge_prices(&current, &mut metadata, incoming).unwrap(),
            current
        );
        assert_eq!(
            merge_prices(&current, &mut metadata, BTreeMap::new()).unwrap(),
            current
        );
    }

    #[test]
    fn automatic_sync_respects_opt_in_interval_new_inventory_and_retry_backoff() {
        let mut metadata = SyncMetadata::default();
        assert!(!due(&metadata, 1, "inventory"));
        metadata.enabled = true;
        assert!(due(&metadata, 1, "inventory"));
        metadata.last_attempt_ms = Some(1);
        metadata.last_sync_ms = Some(1);
        metadata.last_inventory = "inventory".to_owned();
        assert!(!due(&metadata, RETRY_INTERVAL_MS, "new"));
        assert!(due(&metadata, RETRY_INTERVAL_MS + 1, "new"));
        assert!(!due(&metadata, SYNC_INTERVAL_MS, "inventory"));
        assert!(due(&metadata, SYNC_INTERVAL_MS + 1, "inventory"));
    }

    #[test]
    fn inventory_contains_every_offering_and_distinguishes_channel_prices_from_fallbacks() {
        let draft = json!({"upstreams": {
            "first": {"base_url":"https://first.example/v1","models":[{"model":"shared"},{"model":"missing"}]},
            "second": {"base_url":"https://second.example/v1","models":[{"model":"shared"}]}
        }});
        let table = PriceTable::default()
            .next_with_models(BTreeMap::from([
                ("first/shared".to_owned(), row(12, "one").price),
                ("shared".to_owned(), row(99, "generic").price),
                ("old-unconfigured".to_owned(), row(99, "generic").price),
            ]))
            .unwrap();
        let views = offering_views(&draft, &table, &SyncMetadata::default());
        assert_eq!(views.len(), 3);
        let exact = views
            .iter()
            .find(|view| view.key == "first/shared")
            .unwrap();
        assert_eq!(exact.source, "manual");
        assert_eq!(exact.price.unwrap().input_per_mtok, 12);
        let fallback = views
            .iter()
            .find(|view| view.key == "second/shared")
            .unwrap();
        assert_eq!(fallback.source, "fallback");
        assert_eq!(fallback.price.unwrap().input_per_mtok, 99);
        let missing = views
            .iter()
            .find(|view| view.key == "first/missing")
            .unwrap();
        assert_eq!(missing.source, "missing");
        assert!(missing.price.is_none());
    }

    #[test]
    fn inventory_uses_provenance_only_for_matching_channel_and_price() {
        let draft = json!({"upstreams":{"channel":{
            "base_url":"https://one.example/v1","models":[{"model":"a"}]
        }}});
        let identity = provider_identity(&draft["upstreams"]["channel"]);
        let mut metadata = SyncMetadata::default();
        let table = merge_prices(
            &PriceTable::default(),
            &mut metadata,
            BTreeMap::from([("channel/a".to_owned(), row(12, &identity))]),
        )
        .unwrap();
        assert_eq!(
            offering_views(&draft, &table, &metadata)[0].source,
            "provider"
        );
        let mut added_model = draft.clone();
        added_model["upstreams"]["channel"]["models"]
            .as_array_mut()
            .unwrap()
            .push(json!({"model":"b"}));
        assert_eq!(
            offering_views(&added_model, &table, &metadata)[0].source,
            "provider"
        );
        let mut changed = draft;
        changed["upstreams"]["channel"]["base_url"] = json!("https://two.example/v1");
        assert_eq!(
            offering_views(&changed, &table, &metadata)[0].source,
            "manual"
        );
    }

    #[test]
    fn invalid_batch_does_not_commit_prices_or_ownership() {
        let mut metadata = SyncMetadata::default();
        let current = PriceTable::default();
        assert!(merge_prices(
            &current,
            &mut metadata,
            BTreeMap::from([
                ("channel/a".to_owned(), row(12, "one")),
                ("channel/b".to_owned(), row(9_000_000_000_000_001, "one")),
            ])
        )
        .is_err());
        assert!(metadata.records.is_empty());
        assert!(current.models.is_empty());
    }

    #[test]
    fn public_fallback_cannot_replace_a_channel_price_after_channel_failure() {
        let mut metadata = SyncMetadata::default();
        let current = merge_prices(
            &PriceTable::default(),
            &mut metadata,
            BTreeMap::from([("channel/a".to_owned(), row(12, "one"))]),
        )
        .unwrap();
        let mut public = row(20, "one");
        public.source = "models.dev".to_owned();
        let next = merge_prices(
            &current,
            &mut metadata,
            BTreeMap::from([("channel/a".to_owned(), public)]),
        )
        .unwrap();
        assert_eq!(next, current);
        assert_eq!(metadata.records["channel/a"].source, "provider");
    }

    #[test]
    fn disabled_scheduler_and_dirty_config_do_not_contact_channels_or_save_edits() {
        let root =
            std::env::temp_dir().join(format!("price-sync-{}-{}", std::process::id(), now_ms()));
        std::fs::create_dir_all(&root).unwrap();
        let mut draft = template(&root.join("data"), &root.join("plugins"));
        draft.as_object_mut().unwrap().remove("routing");
        let mut inner = AppInner::new(root.join("config.json"), draft, None);
        inner.save_draft().unwrap();
        let state = AppStateManaged(Mutex::new(inner));
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let result = runtime.block_on(synchronize(&state, true)).unwrap();
        assert!(!result.sync.enabled);
        assert!(result.sync.last_attempt_ms.is_none());
        let before = std::fs::read(root.join("config.json")).unwrap();
        {
            let mut inner = state.0.lock().unwrap();
            inner.draft["server"]["listen"] = json!("127.0.0.1:9876");
            inner.observe_draft().unwrap();
        }
        assert!(runtime
            .block_on(synchronize(&state, false))
            .err()
            .unwrap()
            .contains("pending configuration"));
        assert_eq!(before, std::fs::read(root.join("config.json")).unwrap());
        assert!(!state.0.lock().unwrap().price_sync_running);
        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn failed_price_save_preserves_revision_high_water_mark_and_original_draft() {
        let root = std::env::temp_dir().join(format!(
            "price-sync-rollback-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let mut draft = template(&root.join("data"), &root.join("plugins"));
        draft.as_object_mut().unwrap().remove("routing");
        let mut inner = AppInner::new(root.join("config.json"), draft, None);
        inner.save_draft().unwrap();
        let original = inner.draft.clone();
        let previous_revision = inner.config_state.draft_revision();
        std::fs::rename(root.join("config.json"), root.join("config.backup.json")).unwrap();
        std::fs::create_dir(root.join("config.json")).unwrap();
        let pricing = draft_price_table(&inner)
            .unwrap()
            .next_with_model("channel/a", row(12, "one").price)
            .unwrap();
        assert!(save_synced_table(&mut inner, &pricing).is_err());
        assert_eq!(inner.draft, original);
        assert!(!inner.config_state.is_dirty());
        inner.draft["server"]["listen"] = json!("127.0.0.1:9876");
        inner.observe_draft().unwrap();
        assert!(
            inner.config_state.draft_revision() > previous_revision + 1,
            "The failed price revision remains consumed."
        );
        drop(inner);
        std::fs::remove_dir_all(root).unwrap();
    }
}
