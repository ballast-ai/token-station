//! `admin`: the read-only data plane behind `/admin/*`.
//!
//! Serves the same aggregates the desktop shell reads over Tauri IPC — usage
//! stats, the routing table, the plugin listing — so a plain browser (frontend
//! development against a running `serve`) or, later, a remote console can
//! render them without the shell. Everything here derives from the running
//! config, the metrics store and the plugin registry; nothing derives from
//! request content, so the decision-record privacy line is untouched.
//!
//! Deliberately absent: any write. Connecting agents, editing config and
//! touching secrets stay on the desktop's IPC surface, because a loopback
//! HTTP port is reachable by every local process that can present the virtual
//! key, and the blast radius of a read is a number while the blast radius of
//! a write is a user's `~/.claude/settings.json`.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use token_station_router_core::RouterConfig;

use crate::config::{ClientConfig, PluginsConfig};
use crate::plugins::{PluginRegistry, Receipts};
use crate::stats;
use crate::store::SqliteStore;

/// What the admin endpoints answer from: a snapshot taken when `serve`
/// started. The routing table therefore reflects the *running* configuration
/// — which is what a data plane should report — while the desktop's IPC view
/// keeps showing the editable draft.
pub struct AdminContext {
    pub data_dir: PathBuf,
    pub router: RouterConfig,
    pub plugins: PluginsConfig,
}

impl AdminContext {
    /// Builds the read-only snapshot from the same effective Home router that
    /// the Gateway runs. Host-only modes such as Direct must be compiled before
    /// they are exposed through `/admin/router-table`.
    ///
    /// # Errors
    ///
    /// Returns the same fail-closed routing error as Gateway construction when
    /// the effective Home mode cannot be compiled.
    pub fn from_config(config: &ClientConfig) -> Result<Self, String> {
        Ok(Self {
            data_dir: config.data.dir.clone(),
            router: config.home_router_config()?,
            plugins: config.plugins.clone(),
        })
    }

    /// Usage aggregates, shaped exactly like the desktop's `StatsView` so the
    /// frontend needs one type for both transports. A missing metrics store is
    /// `empty: true`, not an error — same contract as the IPC command.
    ///
    /// # Errors
    ///
    /// An unparseable `since` window, an unknown `by` grouping, or an
    /// unreadable store.
    pub fn stats(
        &self,
        since: &str,
        by: Option<&str>,
        agent_id: Option<&str>,
        source: Option<&str>,
        upstream: Option<&str>,
        model: Option<&str>,
    ) -> Result<Value, String> {
        let db = self.data_dir.join("metrics.sqlite");
        if !db.exists() {
            return Ok(json!({
                "total": agg_zero(),
                "groups": [],
                "by": by,
                "empty": true,
            }));
        }
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |duration| {
                u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
            });
        let cutoff = stats::cutoff_from_since(since, now_ms)?;
        let group = match by {
            None | Some("") => None,
            Some("agent") => Some(stats::GroupBy::Agent),
            Some("upstream") => Some(stats::GroupBy::Upstream),
            Some("model") => Some(stats::GroupBy::Model),
            Some("pool") => Some(stats::GroupBy::Pool),
            Some("status") => Some(stats::GroupBy::Status),
            Some("hour") => Some(stats::GroupBy::Hour),
            Some("day") => Some(stats::GroupBy::Day),
            Some("engine") => Some(stats::GroupBy::Engine),
            Some("fallback") => Some(stats::GroupBy::Fallback),
            Some(other) => return Err(format!("unknown grouping `{other}`")),
        };
        let report = stats::collect_filtered(
            &db,
            cutoff,
            None,
            group,
            stats::StatsFilter {
                agent_id,
                source,
                upstream,
                model,
            },
        )?;
        let groups: Vec<Value> = report
            .groups
            .iter()
            .map(|(key, aggregate)| json!([key, agg_view(aggregate)]))
            .collect();
        Ok(json!({
            "total": agg_view(&report.total),
            "groups": groups,
            "by": by,
            "empty": false,
        }))
    }

    /// The newest content-free request receipts. The store enforces the hard
    /// five-row ceiling and returns an empty list when metrics are disabled or
    /// the database has not been created yet.
    ///
    /// # Errors
    ///
    /// An existing metrics store could not be opened, migrated, or read.
    pub fn recent_receipts(&self) -> Result<Value, String> {
        let receipts = SqliteStore::recent_receipts(&self.data_dir.join("metrics.sqlite"), 5)?;
        serde_json::to_value(receipts).map_err(|error| format!("serialize receipts: {error}"))
    }

    /// The four routing layers of the *running* config, shaped like the
    /// desktop's `RouterTableView`.
    #[must_use]
    pub fn router_table(&self) -> Value {
        let heuristic = self.router.heuristic.as_ref();
        let bands: Vec<Value> = heuristic
            .map(|h| {
                h.bands
                    .iter()
                    .map(|band| {
                        let (upstream, model) = self.pool_member(&band.pool);
                        json!({
                            "at_least": band.at_least,
                            "pool": band.pool,
                            "upstream": upstream,
                            "model": model,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let pools: Vec<Value> = self
            .router
            .pools
            .keys()
            .map(|pool| {
                let (upstream, model) = self.pool_member(pool);
                json!({ "pool": pool, "upstream": upstream, "model": model })
            })
            .collect();
        json!({
            "default_pool": self.router.default_pool,
            "assumed_context_window": self.router.assumed_context_window,
            "threshold": heuristic.map(|h| h.threshold),
            "rules": self.router.rules,
            "hint_routes": self.router.hint_routes,
            "bands": bands,
            "pools": pools,
        })
    }

    /// The plugin listing, shaped like the desktop's `PluginsView`. Discovery
    /// runs against the same directory and receipts the CLI `plugin list`
    /// command reads.
    ///
    /// # Errors
    ///
    /// Unreadable receipts or an undiscoverable plugin directory.
    pub fn plugins(&self) -> Result<Value, String> {
        let receipts = Receipts::load(&self.data_dir)?;
        let registry = PluginRegistry::discover(&self.plugins, &receipts)?;
        Ok(json!({
            "dir": self.plugins.dir.display().to_string(),
            "agent": self.plugins.effective_agents().join(", "),
            "dialects": registry.provider_dialects(),
            "listing": registry.render_list(),
        }))
    }

    /// The first member of a pool, as `(upstream, model)` — the operator's
    /// preferred target, which is all the table view names.
    fn pool_member(&self, pool: &str) -> (Option<String>, Option<String>) {
        self.router
            .pools
            .get(pool)
            .and_then(|members| members.first())
            .map_or((None, None), |member| {
                (
                    Some(member.upstream.to_string()),
                    Some(member.model.clone()),
                )
            })
    }
}

fn agg_view(aggregate: &stats::Aggregate) -> Value {
    json!({
        "requests": aggregate.requests,
        "errors": aggregate.errors,
        "p50_latency_ms": aggregate.p50_latency_ms,
        "p95_latency_ms": aggregate.p95_latency_ms,
        "input_tokens": aggregate.input_tokens,
        "output_tokens": aggregate.output_tokens,
        "cache_read_tokens": aggregate.cache_read_tokens,
        "cache_write_tokens": aggregate.cache_write_tokens,
        "reasoning_tokens": aggregate.reasoning_tokens,
        "cost_micros": aggregate.cost_micros,
        "priced_requests": aggregate.priced_requests,
        "unpriced_requests": aggregate.unpriced_requests,
        "legacy_input_requests": aggregate.legacy_input_requests,
    })
}

fn agg_zero() -> Value {
    json!({
        "requests": 0,
        "errors": 0,
        "p50_latency_ms": 0,
        "p95_latency_ms": 0,
        "input_tokens": 0,
        "output_tokens": 0,
        "cache_read_tokens": 0,
        "cache_write_tokens": 0,
        "reasoning_tokens": 0,
        "cost_micros": null,
        "priced_requests": 0,
        "unpriced_requests": 0,
        "legacy_input_requests": 0,
    })
}

#[cfg(test)]
mod tests {
    use super::{agg_view, agg_zero};

    #[test]
    fn both_stats_transports_expose_legacy_usage_semantics() {
        let aggregate = crate::stats::Aggregate {
            legacy_input_requests: 3,
            ..crate::stats::Aggregate::default()
        };

        assert_eq!(agg_view(&aggregate)["legacy_input_requests"], 3);
        assert_eq!(agg_zero()["legacy_input_requests"], 0);
    }
}
