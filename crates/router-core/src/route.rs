use serde::{Deserialize, Serialize};
use token_station_protocol::{AgentHint, ChatRequest, ModelCapability};

use crate::config::{ConfigError, RouterConfig};
use crate::decision::{DecidedBy, Decision, NoRoute, UnmetRequirement, UpstreamModel};
use crate::features::RequestFeatures;

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
}

impl Candidate {
    #[must_use]
    pub fn new(target: UpstreamModel, capability: ModelCapability, health: Health) -> Self {
        Self {
            target,
            capability,
            health,
        }
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

        let ranked = self.rank(members, candidates, &features, pool)?;
        let (chosen, fallbacks) = ranked.split_first().expect("rank returns a non-empty list");

        Ok(Decision {
            chosen: (*chosen).clone(),
            decided_by,
            fallbacks: fallbacks.iter().map(|target| (*target).clone()).collect(),
            features,
            pool: pool.to_owned(),
        })
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
        members: &'a [UpstreamModel],
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

        if features.tool_count > 0 && !capability.tool {
            return Some(UnmetRequirement::Tools);
        }
        if features.has_images && !capability.vision {
            return Some(UnmetRequirement::Vision);
        }
        if features.requires_json_schema && !capability.json_schema {
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
