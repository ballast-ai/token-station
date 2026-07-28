use serde::{Deserialize, Serialize};
use token_station_protocol::{AgentHint, ChatRequest, ModelCapability};

use crate::config::{ConfigError, RecoveryPolicy, RouterConfig, RoutingMode};
use crate::decision::{DecidedBy, Decision, NoRoute, UnmetRequirement, UpstreamModel};
use crate::features::RequestFeatures;
use crate::quota::{QuotaConfig, QuotaState, quota_rank};

/// The `pool` name a quota-first decision carries. Quota mode has no tiers, but
/// [`Decision::pool`] is not optional, and a stable label keeps receipts and the
/// quota viewer able to say "this went through quota routing".
const QUOTA_POOL: &str = "quota";

/// Whether an upstream is currently worth sending a request to.
///
/// Supplied by the host, not computed here: deciding that an upstream is sick
/// needs a clock and a history of failures, and this crate has neither. The
/// simple health check of `C1#2` — count timeouts and errors, take the upstream
/// out, probe it back in — produces exactly this value.
///
/// Ordering is meaningful: `Healthy` sorts before `Degraded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Health {
    Healthy,
    /// Reachable, but being probed back after failures. Used only if nothing
    /// healthy remains.
    Degraded,
    /// Out of rotation. Never routed to.
    Unavailable,
}

/// One upstream-and-model the host currently has, with what it can do.
///
/// `capability` comes from the provider adapter's `model_capabilities`, and
/// `health` from the host's health checker. The router owns neither.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub target: UpstreamModel,
    pub capability: ModelCapability,
    pub health: Health,
    /// Whether this upstream runs on the local machine. The host sets it from
    /// the upstream's `local` declaration; the router uses it to honor a
    /// `local_only` route without the data ever leaving the box.
    pub local: bool,
    /// This account's quota picture, host-measured. Read only by
    /// [`Router::route_quota_first`]; ignored in tiered mode. Defaults to a
    /// non-windowed, fully-open account.
    pub quota: QuotaState,
}

impl Candidate {
    #[must_use]
    pub fn new(target: UpstreamModel, capability: ModelCapability, health: Health) -> Self {
        Self {
            target,
            capability,
            health,
            local: false,
            quota: QuotaState::default(),
        }
    }

    /// Marks this candidate as a local upstream, for `local_only` routing.
    #[must_use]
    pub fn local(mut self, local: bool) -> Self {
        self.local = local;
        self
    }

    /// Attaches this account's measured quota state, for quota-first routing.
    #[must_use]
    pub fn quota(mut self, quota: QuotaState) -> Self {
        self.quota = quota;
        self
    }
}

/// A validated [`RouterConfig`], and the only thing that can route.
///
/// Making this a distinct type is what lets [`Decision`] have no "unknown pool"
/// failure mode: a pool named by a rule is known to exist before any request
/// arrives, because [`Router::new`] refused the config otherwise.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Router {
    config: RouterConfig,
}

impl Router {
    /// # Errors
    ///
    /// Returns the [`ConfigError`] that makes this configuration unroutable.
    pub fn new(config: RouterConfig) -> Result<Self, ConfigError> {
        config.validate()?;
        Ok(Self { config })
    }

    /// Which routing philosophy this router runs. The host reads it to dispatch
    /// between [`Self::route`] (tiered) and [`Self::route_quota_first`].
    #[must_use]
    pub fn routing_mode(&self) -> RoutingMode {
        self.config.routing_mode
    }

    /// Quota-first tuning (weights, tie band). Meaningful only in quota mode.
    #[must_use]
    pub fn quota_config(&self) -> &QuotaConfig {
        &self.config.quota
    }

    #[must_use]
    pub fn config(&self) -> &RouterConfig {
        &self.config
    }

    /// Picks where this request goes.
    ///
    /// Pure: no clock, no randomness, no IO. Two consequences that the design
    /// depends on. The audit command (`C3#3`) can replay a logged decision
    /// against the config of the day and get the same answer. And the local
    /// client and the server gateway, given the same config and the same
    /// candidates, cannot disagree — which is the fork this crate exists to
    /// prevent.
    ///
    /// The request's content is read (a keyword rule scans it, the heuristic
    /// counts code fences) and then dropped. Nothing content-derived survives
    /// into the returned [`Decision`] except numbers.
    ///
    /// # Errors
    ///
    /// [`NoRoute`] when the selected pool holds nothing this request can use.
    /// Its [`NoRoute::error_code`] says whether another upstream is worth trying.
    ///
    /// # Panics
    ///
    /// Never, for a [`Router`] obtained from [`Router::new`]: validation proved
    /// every pool a layer can select exists, and `rank` returns either a
    /// non-empty list or an error. The `expect`s below are those two facts
    /// written down.
    pub fn route(
        &self,
        request: &ChatRequest,
        hints: &[AgentHint],
        candidates: &[Candidate],
    ) -> Result<Decision, NoRoute> {
        let features = RequestFeatures::extract(request, hints);

        // Exact-model Agents pin the caller's model instead of tier-routing it.
        if self.config.honor_exact_model {
            return self.route_exact(request, features, candidates);
        }

        let (pool, decided_by) = self.select_pool(request, hints, &features);

        let members = self
            .config
            .pools
            .get(pool)
            .expect("Router::new proved every referenced pool exists");

        let mut targets = self.rank(members, candidates, &features, pool)?;

        // Reliability, decided separately from quality: the selected tier's
        // candidates come first; only an explicit `Ordered` policy appends
        // other tiers' candidates behind them as authorized backups.
        if let RecoveryPolicy::Ordered { pools } = &self.config.recovery {
            self.extend_with_backups(&mut targets, pools, pool, candidates, &features);
        }

        let (chosen, fallbacks) = targets
            .split_first()
            .expect("rank returns a non-empty list");

        Ok(Decision {
            chosen: (*chosen).clone(),
            decided_by,
            fallbacks: fallbacks.iter().map(|target| (*target).clone()).collect(),
            features,
            pool: pool.to_owned(),
        })
    }

    /// Quota-first routing: ignore request difficulty and rank the caller's
    /// quota-bearing accounts so the one whose allowance is closing soonest is
    /// served first, keeping a conversation on the account it warmed unless that
    /// account is rate-pressured. `last_used` is the account this conversation
    /// was last routed to (prompt-cache affinity), or `None` for a fresh one.
    ///
    /// The full ranked order becomes the decision's chosen + fallbacks, so
    /// failover simply walks down it. Capability is still enforced — a request
    /// needing tools skips a model that lacks them — because that is
    /// correctness, not the quality gating quota mode deliberately drops.
    ///
    /// # Errors
    ///
    /// [`NoRoute::Unsatisfiable`] when no supplied account can serve the request
    /// at all; [`NoRoute::Unavailable`] when accounts could serve it but are all
    /// out of rotation or have spent their allowance.
    ///
    /// # Panics
    ///
    /// Never: the `expect` fires only if `ranked` is empty, which the guard
    /// above returns `Unavailable` for before reaching it.
    pub fn route_quota_first(
        &self,
        request: &ChatRequest,
        candidates: &[Candidate],
        last_used: Option<&UpstreamModel>,
    ) -> Result<Decision, NoRoute> {
        let features = RequestFeatures::extract(request, &[]);
        let cfg = &self.config.quota;

        // Capability gate first: correctness, not quality. Report the first
        // unmet requirement if nothing can serve, matching tiered routing.
        let capable: Vec<&Candidate> = candidates
            .iter()
            .filter(|candidate| self.satisfies(candidate, &features).is_none())
            .collect();
        if capable.is_empty() {
            let reason = candidates
                .first()
                .and_then(|candidate| self.satisfies(candidate, &features))
                .unwrap_or(UnmetRequirement::Unknown);
            return Err(NoRoute::Unsatisfiable {
                pool: QUOTA_POOL.to_owned(),
                reason,
            });
        }

        // Rank the capable, in-rotation, non-exhausted accounts by quota. The
        // index is the candidate's position in the operator's order, used only
        // to break exact ties (earlier-added wins).
        let mut ranked: Vec<(&Candidate, _)> = capable
            .iter()
            .copied()
            .enumerate()
            .filter(|(_, candidate)| candidate.health != Health::Unavailable)
            .filter_map(|(index, candidate)| {
                let is_last_used = last_used == Some(&candidate.target);
                let index = u32::try_from(index).unwrap_or(u32::MAX);
                quota_rank(
                    &candidate.quota,
                    is_last_used,
                    index,
                    &cfg.weights,
                    cfg.reset_tie_band_ms,
                )
                .map(|rank| (candidate, rank))
            })
            .collect();

        if ranked.is_empty() {
            // Capable candidates exist but all are out of rotation or exhausted.
            return Err(NoRoute::Unavailable {
                pool: QUOTA_POOL.to_owned(),
            });
        }

        // Best first (a `QuotaRank` compares greater when preferred). Ranks
        // embed the insertion index, so the order is total and deterministic.
        ranked.sort_by_key(|(_, rank)| std::cmp::Reverse(*rank));

        let targets: Vec<&UpstreamModel> = ranked
            .iter()
            .map(|(candidate, _)| &candidate.target)
            .collect();
        let (chosen, fallbacks) = targets.split_first().expect("ranked is non-empty");

        Ok(Decision {
            chosen: (*chosen).clone(),
            decided_by: DecidedBy::Quota,
            fallbacks: fallbacks.iter().map(|target| (*target).clone()).collect(),
            features,
            pool: QUOTA_POOL.to_owned(),
        })
    }

    /// Appends backup-tier candidates after the selected tier's, in the order the
    /// operator authorized, health-ranked and de-duplicated. A backup tier that
    /// has nothing usable contributes nothing rather than failing the route — the
    /// selected tier already produced a routable chosen target.
    fn extend_with_backups<'a>(
        &self,
        targets: &mut Vec<&'a UpstreamModel>,
        pools: &[String],
        selected: &str,
        candidates: &'a [Candidate],
        features: &RequestFeatures,
    ) {
        for backup in pools {
            if backup == selected {
                continue;
            }
            let Some(members) = self.config.pools.get(backup) else {
                continue;
            };
            if let Ok(ranked) = self.rank(members, candidates, features, backup) {
                for target in ranked {
                    if !targets.contains(&target) {
                        targets.push(target);
                    }
                }
            }
        }
    }

    /// Honor the caller's exact model: serve only candidates whose wire model is
    /// the one named, ordered by health so failover stays within that same model
    /// across whichever providers carry it. Refuse — never substitute — when none
    /// can. The `pool` in the decision is the pinned model name, for explaining a
    /// route the same way tier routing does.
    fn route_exact(
        &self,
        request: &ChatRequest,
        features: RequestFeatures,
        candidates: &[Candidate],
    ) -> Result<Decision, NoRoute> {
        let named: Vec<&Candidate> = candidates
            .iter()
            .filter(|candidate| candidate.target.model == request.model)
            .collect();

        if named.is_empty() {
            return Err(NoRoute::Unsatisfiable {
                pool: request.model.clone(),
                reason: UnmetRequirement::ExactModelUnavailable,
            });
        }

        let capable: Vec<&Candidate> = named
            .iter()
            .copied()
            .filter(|candidate| self.satisfies(candidate, &features).is_none())
            .collect();

        if capable.is_empty() {
            let reason = self
                .satisfies(named[0], &features)
                .unwrap_or(UnmetRequirement::Unknown);
            return Err(NoRoute::Unsatisfiable {
                pool: request.model.clone(),
                reason,
            });
        }

        let mut usable: Vec<&Candidate> = capable
            .into_iter()
            .filter(|candidate| candidate.health != Health::Unavailable)
            .collect();

        if usable.is_empty() {
            return Err(NoRoute::Unavailable {
                pool: request.model.clone(),
            });
        }

        usable.sort_by_key(|candidate| candidate.health);

        let targets: Vec<&UpstreamModel> =
            usable.iter().map(|candidate| &candidate.target).collect();
        let (chosen, fallbacks) = targets.split_first().expect("usable is non-empty");

        Ok(Decision {
            chosen: (*chosen).clone(),
            decided_by: DecidedBy::ExactModel {
                model: request.model.clone(),
            },
            fallbacks: fallbacks.iter().map(|target| (*target).clone()).collect(),
            features,
            pool: request.model.clone(),
        })
    }

    /// Layer 1, then 2, then 3, then the default. First to answer wins.
    fn select_pool<'config>(
        &'config self,
        request: &ChatRequest,
        hints: &[AgentHint],
        features: &RequestFeatures,
    ) -> (&'config str, DecidedBy) {
        for rule in &self.config.rules {
            if rule.matcher.matches(features, request) {
                return (
                    &rule.route_to,
                    DecidedBy::Rule {
                        rule: rule.id.clone(),
                    },
                );
            }
        }

        for route in &self.config.hint_routes {
            if hints
                .iter()
                .any(|hint| hint.kind == route.kind && hint.value == route.value)
            {
                return (
                    &route.route_to,
                    DecidedBy::Hint {
                        kind: route.kind,
                        // The configured key, not the header's value. They are
                        // equal here; taking this one means an agent can never
                        // write its own text into a persisted decision record.
                        value: route.value.clone(),
                    },
                );
            }
        }

        if let Some(heuristic) = &self.config.heuristic {
            let score = heuristic.score(features);
            // `threshold` in the record is the floor of the tier that fired —
            // the band's `at_least` under banding, the single threshold under
            // the binary split.
            let (pool, threshold) = heuristic.select(score);
            return (pool, DecidedBy::Heuristic { score, threshold });
        }

        (&self.config.default_pool, DecidedBy::Default)
    }

    /// Drops what cannot serve the request, then orders by health.
    fn rank<'a>(
        &self,
        members: &[UpstreamModel],
        candidates: &'a [Candidate],
        features: &RequestFeatures,
        pool: &str,
    ) -> Result<Vec<&'a UpstreamModel>, NoRoute> {
        let installed: Vec<&Candidate> = members
            .iter()
            .filter_map(|member| candidates.iter().find(|c| c.target == *member))
            .collect();

        if installed.is_empty() {
            return Err(NoRoute::Unsatisfiable {
                pool: pool.to_owned(),
                reason: UnmetRequirement::Unknown,
            });
        }

        // Local-only routing keeps the data on the machine: try the pool's local
        // candidates first, and only reach for cloud when no local one can serve
        // the request *and* the operator authorized it. Off by default, so an
        // ordinary route runs the full-candidate path below unchanged.
        if self.config.local_only {
            let local: Vec<&Candidate> = installed.iter().copied().filter(|c| c.local).collect();
            if local.is_empty() {
                // Nothing local is installed for this pool. Without explicit
                // authorization the request is refused rather than sent to cloud.
                if !self.config.allow_cloud_fallback {
                    return Err(NoRoute::Unsatisfiable {
                        pool: pool.to_owned(),
                        reason: UnmetRequirement::LocalOnly,
                    });
                }
            } else {
                match self.usable_targets(&local, features, pool) {
                    Ok(targets) => return Ok(targets),
                    // Local candidates exist but none can serve or all are ejected.
                    // Leave the box only when cloud fallback was authorized.
                    Err(err) => {
                        if !self.config.allow_cloud_fallback {
                            return Err(err);
                        }
                    }
                }
            }
        }

        self.usable_targets(&installed, features, pool)
    }

    /// Drops what cannot serve the request and what is out of rotation, then
    /// orders the rest by health. The shared tail of `rank`, called once for the
    /// full pool and — under `local_only` — once for its local subset first.
    fn usable_targets<'a>(
        &self,
        installed: &[&'a Candidate],
        features: &RequestFeatures,
        pool: &str,
    ) -> Result<Vec<&'a UpstreamModel>, NoRoute> {
        let capable: Vec<&Candidate> = installed
            .iter()
            .copied()
            .filter(|candidate| self.satisfies(candidate, features).is_none())
            .collect();

        if capable.is_empty() {
            // Report the first thing missing from the first installed candidate:
            // a list of every unmet requirement across every candidate reads as
            // noise, and the operator fixes them one at a time anyway.
            let reason = self
                .satisfies(installed[0], features)
                .unwrap_or(UnmetRequirement::Unknown);
            return Err(NoRoute::Unsatisfiable {
                pool: pool.to_owned(),
                reason,
            });
        }

        let mut usable: Vec<&Candidate> = capable
            .into_iter()
            .filter(|candidate| candidate.health != Health::Unavailable)
            .collect();

        if usable.is_empty() {
            return Err(NoRoute::Unavailable {
                pool: pool.to_owned(),
            });
        }

        // Stable, so the operator's order survives inside a health class.
        usable.sort_by_key(|candidate| candidate.health);

        Ok(usable
            .into_iter()
            .map(|candidate| &candidate.target)
            .collect())
    }

    /// `None` when the candidate can serve the request; otherwise the first
    /// thing it cannot do.
    fn satisfies(
        &self,
        candidate: &Candidate,
        features: &RequestFeatures,
    ) -> Option<UnmetRequirement> {
        let capability = &candidate.capability;

        if features.tool_count > 0 && !capability.tool_state().is_supported() {
            return Some(UnmetRequirement::Tools);
        }
        if features.has_images && !capability.vision_state().is_supported() {
            return Some(UnmetRequirement::Vision);
        }
        if features.requires_json_schema && !capability.json_schema_state().is_supported() {
            return Some(UnmetRequirement::JsonSchema);
        }

        // Zero means the adapter did not report one. Treating that as unlimited
        // would send a 200k-token request at a model that silently truncates it.
        let window = if capability.context_window == 0 {
            self.config.assumed_context_window
        } else {
            capability.context_window
        };
        let needed = features
            .estimated_input_tokens
            .saturating_add(features.requested_max_output_tokens.unwrap_or(0));
        if needed > window {
            return Some(UnmetRequirement::ContextWindow { needed });
        }

        None
    }
}
