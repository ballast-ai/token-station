use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use token_station_protocol::{ChatRequest, Content, ContentPart, HintKind};

use crate::quota::QuotaConfig;
use crate::{RequestFeatures, UpstreamModel};

/// The only config version this crate understands.
pub const CONFIG_VERSION: u32 = 1;

/// What a model's capability report is assumed to be when it does not say.
///
/// `ModelCapability::context_window == 0` means unknown, and the protocol docs
/// say the router must then refuse long-context requests. Refusing *everything*
/// would make a self-hosted upstream unroutable; refusing *nothing* would send a
/// 200k-token request at a model that will drop half of it. So an unknown window
/// is treated as this many tokens, and the operator can raise it.
const DEFAULT_ASSUMED_CONTEXT_WINDOW: u32 = 8_192;

/// The routing configuration, as it appears in a file or a database row.
///
/// Untrusted-ish input: an operator wrote it, possibly a remote config service
/// pushed it. So it parses into this, and [`crate::Router::new`] is the only way
/// to get something routable out of it.
///
/// `deny_unknown_fields` throughout is not pedantry. A misspelled `requires_tols`
/// under a rule would deserialize into an all-`None` predicate, which matches
/// *every* request — the rule would fire on everything and every rule after it
/// would be dead. Silently. The same class of bug the manifest schema was fixed
/// for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RouterConfig {
    pub version: u32,
    /// Named, ordered lists of places a request can go. Order is the operator's
    /// preference; the router only reorders by health. Empty is legal only in
    /// quota-first mode (tiered validation rejects it).
    #[serde(default)]
    pub pools: BTreeMap<String, Vec<UpstreamModel>>,
    /// Layer 1. Checked in order; the first match wins and routing stops.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rules: Vec<Rule>,
    /// Layer 2. Consulted only when no rule matched.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hint_routes: Vec<HintRoute>,
    /// Layer 3. Consulted only when no rule and no hint matched.
    ///
    /// The learned classifier of `C3#2` replaces this layer, and until it ships
    /// this *is* that layer, per the routing design's staged rollout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub heuristic: Option<Heuristic>,
    /// Where a request goes when nothing above decided. Empty is legal only in
    /// quota-first mode (tiered validation rejects it).
    #[serde(default)]
    pub default_pool: String,
    #[serde(default = "assumed_context_window_default")]
    pub assumed_context_window: u32,
    /// Honor the caller's exact model name instead of tier-routing it.
    ///
    /// Off by default: the request's `model` is a routing input and the tier
    /// decides the actual model. On (a per-Agent choice), a named model is a
    /// pin — the router serves only that same model, failing over across the
    /// providers that carry it, and refuses rather than silently substituting a
    /// different one. This is what makes "I asked for `deepseek-chat`" mean it.
    #[serde(default)]
    pub honor_exact_model: bool,
    /// What may serve as a backup once the selected tier's candidates are all
    /// exhausted. The reliability half of routing, kept separate from tier
    /// selection: the tier decides *quality*, this decides *what happens when it
    /// fails*.
    #[serde(default)]
    pub recovery: RecoveryPolicy,
    /// Lock every route to upstreams declared `local`: the request must not
    /// leave the machine. Off by default, so an ordinary config is unchanged.
    /// When on, a pool's non-local candidates are skipped, and a request that no
    /// local candidate can serve is refused — unless [`Self::allow_cloud_fallback`]
    /// explicitly authorizes leaving the box.
    #[serde(default)]
    pub local_only: bool,
    /// Under [`Self::local_only`], permit falling back to non-local (cloud)
    /// candidates when no local one can serve the request. The privacy-vs-
    /// availability escape hatch: off means local-or-nothing (fail closed), on
    /// means try local first, then cloud. Ignored unless `local_only` is set.
    #[serde(default)]
    pub allow_cloud_fallback: bool,
    /// Which routing philosophy is active. Tiered (the default) routes by
    /// request difficulty through the pools above; quota-first ignores
    /// difficulty and drains the caller's quota-bearing accounts before they
    /// refresh (see [`crate::Router::route_quota_first`]). The two are mutually
    /// exclusive top-level modes.
    #[serde(default)]
    pub routing_mode: RoutingMode,
    /// Tuning for quota-first routing. Ignored in tiered mode. Fully defaulted,
    /// so flipping the mode on needs nothing else.
    #[serde(default)]
    pub quota: QuotaConfig,
    /// The ordered accounts (upstream + model) that participate in quota-first
    /// rotation. Order is the operator's priority: it breaks exact quota ties
    /// (earlier wins). When non-empty in quota-first mode, only these accounts
    /// route, and only in this order; empty falls back to every host-supplied
    /// candidate (the pre-selection behavior). Ignored in tiered mode.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub quota_accounts: Vec<UpstreamModel>,
}

/// The active routing philosophy. The two modes are mutually exclusive: tiered
/// routes by request difficulty, quota-first by which allowance is closing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingMode {
    /// Route by request difficulty through the tier pools. The default.
    #[default]
    Tiered,
    /// Route by quota: drain the account whose window is closing soonest,
    /// ignoring request difficulty. Pools/rules/hints/heuristic are unused.
    QuotaFirst,
}

/// Failover behavior once the selected tier is exhausted, decoupled from which
/// tier was chosen. Naming this separately stops "pick the good tier" and "the
/// good tier broke" from collapsing into one opaque outcome.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case")]
pub enum RecoveryPolicy {
    /// Stay inside the selected tier; if its candidates are all exhausted, fail.
    /// The honest default — nothing is downgraded unless the operator asked.
    #[default]
    Strict,
    /// After the selected tier, try these tiers in order. Listing them is the
    /// operator's explicit cross-tier authorization; a route never silently
    /// leaves its tier without one.
    Ordered { pools: Vec<String> },
}

const fn assumed_context_window_default() -> u32 {
    DEFAULT_ASSUMED_CONTEXT_WINDOW
}

/// A rule the operator wrote. Highest priority, always.
///
/// This is the overrideable fallback in the transparency design: whatever the heuristic or
/// a future classifier would have chosen, a rule overrides it, and the decision
/// records which rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Stable across edits; it is what lands in the decision record and in the
    /// cloud-sync whitelist (the matched routing-rule ID).
    pub id: String,
    #[serde(rename = "when")]
    pub matcher: Match,
    pub route_to: String,
}

/// Predicates that must *all* hold. An empty match is refused at validation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Match {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_input_tokens_at_least: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub estimated_input_tokens_below: Option<u32>,
    /// `true` requires tools, `false` requires their absence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_tools: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_json_schema: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_vision: Option<bool>,
    /// Matches the model the caller asked for, exactly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_model: Option<String>,
    /// At least this many reasoning-marker hits (crate lexicon, both
    /// languages). This is where V1's "2+ markers force the reasoning tier"
    /// special case now lives: as an operator-visible rule, not a hidden
    /// branch inside the score.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_markers_at_least: Option<u32>,
    /// Any one of these, case-insensitive, anywhere in the message text.
    ///
    /// This predicate reads the prompt. That is fine and it is local — what it
    /// must never do is remember. A keyword hit produces `DecidedBy::Rule { id }`
    /// and nothing else, so the record cannot reveal that the request said "prove".
    /// One bit per configured keyword would still be a content channel.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub keywords_any: Vec<String>,
}

impl Match {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.estimated_input_tokens_at_least.is_none()
            && self.estimated_input_tokens_below.is_none()
            && self.requires_tools.is_none()
            && self.requires_json_schema.is_none()
            && self.requires_vision.is_none()
            && self.requested_model.is_none()
            && self.reasoning_markers_at_least.is_none()
            && self.keywords_any.is_empty()
    }

    pub(crate) fn matches(&self, features: &RequestFeatures, request: &ChatRequest) -> bool {
        let tokens = features.estimated_input_tokens;

        let refuted = self
            .estimated_input_tokens_at_least
            .is_some_and(|floor| tokens < floor)
            || self
                .estimated_input_tokens_below
                .is_some_and(|ceiling| tokens >= ceiling)
            || self
                .requires_tools
                .is_some_and(|wanted| (features.tool_count > 0) != wanted)
            || self
                .requires_json_schema
                .is_some_and(|wanted| features.requires_json_schema != wanted)
            || self
                .requires_vision
                .is_some_and(|wanted| features.has_images != wanted)
            || self
                .requested_model
                .as_ref()
                .is_some_and(|model| request.model != *model)
            || self
                .reasoning_markers_at_least
                .is_some_and(|floor| features.reasoning_marker_count < floor)
            || (!self.keywords_any.is_empty() && !mentions_any(request, &self.keywords_any));

        !refuted
    }
}

fn mentions_any(request: &ChatRequest, keywords: &[String]) -> bool {
    let needles: Vec<String> = keywords.iter().map(|word| word.to_lowercase()).collect();

    message_texts(request).any(|text| {
        let haystack = text.to_lowercase();
        needles.iter().any(|needle| haystack.contains(needle))
    })
}

fn message_texts(request: &ChatRequest) -> impl Iterator<Item = &str> {
    request
        .messages
        .iter()
        .filter_map(|message| message.content.as_ref())
        .flat_map(|content| -> Box<dyn Iterator<Item = &str>> {
            match content {
                Content::Text(text) => Box::new(std::iter::once(text.as_str())),
                Content::Parts(parts) => Box::new(parts.iter().filter_map(|part| match part {
                    ContentPart::Text { text } => Some(text.as_str()),
                    // 0.3.0: reasoning/unmodeled parts stay out of context
                    // summaries — same rationale as feature extraction.
                    ContentPart::ImageUrl { .. }
                    | ContentPart::Thinking { .. }
                    | ContentPart::RedactedThinking { .. }
                    | ContentPart::Unknown(_) => None,
                })),
            }
        })
}

/// Layer 2: the calling agent knows what step it is on, and the host does not.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HintRoute {
    pub kind: HintKind,
    pub value: String,
    pub route_to: String,
}

/// Layer 3: score the request, compare to a threshold, pick a pool.
///
/// Integer arithmetic throughout. A float score would let the local client and
/// the server gateway disagree in the last bit and route the same request to
/// different models, which is precisely the fork this crate exists to prevent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Heuristic {
    pub weights: Weights,
    pub threshold: u32,
    /// Where a request scoring `>= threshold` goes.
    pub above: String,
    /// Where everything else goes.
    pub below: String,
    /// Multi-tier banding (V2-P2-3). Checked in order, first band whose
    /// `at_least <= score` wins; validation demands strictly descending
    /// `at_least` so the order on disk IS the precedence. Empty = the binary
    /// above/below split — every pre-0.2 profile keeps its behavior. A band
    /// list ending in `at_least = 0` never falls through; one that does not
    /// falls back to the binary split.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub bands: Vec<Band>,
}

/// One tier of a banded heuristic: scores of at least `at_least` land in
/// `pool` (unless a higher band already claimed them).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Band {
    pub at_least: u32,
    pub pool: String,
}

impl Heuristic {
    /// The complexity score, saturating rather than wrapping.
    ///
    /// A request that overflows a `u32` of score is, for every purpose the
    /// threshold has, maximally complex.
    #[must_use]
    pub fn score(&self, features: &RequestFeatures) -> u32 {
        let weights = &self.weights;
        // Score difficulty from conversation content, excluding the Agent's fixed
        // system-prompt scaffolding. Counting thousands of system tokens would
        // push every request into the high tier; see conversation_tokens.
        let mut score = features
            .conversation_tokens
            .saturating_div(weights.tokens_per_point.max(1));

        // advertised tool_count is fixed per Agent. OpenCode sends its full tool
        // set on every request regardless of difficulty, so exclude it from the
        // difficulty score. Previously tool_count * per_tool could push a simple
        // greeting into the high tier. tool_count still gates capabilities so
        // tool requests reach models that support calls; per_tool remains stored
        // but no longer contributes to scoring.
        score = score.saturating_add(
            features
                .code_block_count
                .saturating_mul(weights.per_code_block),
        );
        score = score.saturating_add(
            features
                .message_count
                .saturating_sub(1)
                .saturating_mul(weights.per_extra_turn),
        );
        if features.requires_json_schema {
            score = score.saturating_add(weights.json_schema);
        }
        if features.has_images {
            score = score.saturating_add(weights.image);
        }
        score = score.saturating_add(
            features
                .reasoning_marker_count
                .saturating_mul(weights.per_reasoning_marker),
        );
        score = score.saturating_add(
            features
                .technical_term_count
                .saturating_mul(weights.per_technical_term),
        );
        score = score.saturating_add(
            features
                .code_keyword_count
                .saturating_mul(weights.per_code_keyword),
        );
        score = score.saturating_add(
            features
                .math_term_count
                .saturating_mul(weights.per_math_term),
        );
        score = score.saturating_add(
            features
                .creative_term_count
                .saturating_mul(weights.per_creative_term),
        );
        score = score.saturating_add(
            features
                .multi_step_signal
                .saturating_mul(weights.per_multi_step_point),
        );
        score = score.saturating_add(features.question_count.saturating_mul(weights.per_question));
        if features.system_format_hint {
            score = score.saturating_add(weights.system_format);
        }
        if weights.max_output_tokens_per_point > 0 {
            score = score.saturating_add(
                features
                    .requested_max_output_tokens
                    .unwrap_or(0)
                    .saturating_div(weights.max_output_tokens_per_point),
            );
        }
        // The one subtractive dimension, last, saturating at zero: easy-talk
        // hits discount the total but can never make it negative.
        score.saturating_sub(
            features
                .simple_indicator_count
                .saturating_mul(weights.per_simple_indicator),
        )
    }

    /// The pool this score lands in, and the floor of the tier it hit (which
    /// is what `DecidedBy::Heuristic.threshold` records).
    pub(crate) fn select(&self, score: u32) -> (&str, u32) {
        for band in &self.bands {
            if score >= band.at_least {
                return (&band.pool, band.at_least);
            }
        }
        if score >= self.threshold {
            (&self.above, self.threshold)
        } else {
            (&self.below, self.threshold)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Weights {
    /// One point per this many estimated input tokens. Must not be zero.
    pub tokens_per_point: u32,
    pub per_tool: u32,
    pub json_schema: u32,
    pub image: u32,
    pub per_code_block: u32,
    /// Conversation depth: charged per turn after the first.
    pub per_extra_turn: u32,
    /// Dims 2–10 of the ported V1 complexity algorithm (V2-P2-3). All default
    /// to 0, which reproduces the pre-0.2 six-dimension score exactly — an
    /// existing profile keeps routing byte-for-byte the same until its
    /// operator opts in.
    #[serde(default)]
    pub per_reasoning_marker: u32,
    #[serde(default)]
    pub per_technical_term: u32,
    /// SUBTRACTS. Hits on the easy-request vocabulary argue the request down;
    /// the score saturates at zero rather than going negative.
    #[serde(default)]
    pub per_simple_indicator: u32,
    #[serde(default)]
    pub per_code_keyword: u32,
    #[serde(default)]
    pub per_math_term: u32,
    #[serde(default)]
    pub per_creative_term: u32,
    /// Charged per point of `multi_step_signal` (0, 6, or 10).
    #[serde(default)]
    pub per_multi_step_point: u32,
    #[serde(default)]
    pub per_question: u32,
    /// Flat bonus when structured-output vocabulary appears in system text.
    #[serde(default)]
    pub system_format: u32,
    /// One point per this many requested max output tokens (0 = dimension
    /// off). V1's ">100K `max_tokens` escalates" special case, expressed as a
    /// weight instead of a branch.
    #[serde(default)]
    pub max_output_tokens_per_point: u32,
}

impl RouterConfig {
    /// Everything that must hold before a request may be routed with this.
    ///
    /// # Errors
    ///
    /// Returns the first [`ConfigError`]. Order is deliberate — version, then
    /// pools, then the layers that reference them — so a config naming a pool
    /// that does not exist is not first reported as a bad rule.
    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if self.version != CONFIG_VERSION {
            return Err(ConfigError::UnsupportedVersion(self.version));
        }
        if self.assumed_context_window == 0 {
            return Err(ConfigError::AssumedContextWindowIsZero);
        }

        // Quota-first mode uses none of the tier machinery — pools, the default
        // pool, rules, hints, the heuristic, ordered recovery. Its accounts are
        // the candidates the host supplies at route time, ranked by quota, so a
        // quota-first config legitimately carries empty pools and an empty
        // default pool. Skip the tiered validation entirely rather than force a
        // dummy pool into existence.
        if self.routing_mode == RoutingMode::QuotaFirst {
            return Ok(());
        }

        if self.pools.is_empty() {
            return Err(ConfigError::NoPools);
        }
        for (name, members) in &self.pools {
            if members.is_empty() {
                return Err(ConfigError::EmptyPool(name.clone()));
            }
        }

        self.require_pool(&self.default_pool, "default_pool")?;

        let mut seen_rules = BTreeSet::new();
        for rule in &self.rules {
            if rule.id.is_empty() {
                return Err(ConfigError::EmptyRuleId);
            }
            if !seen_rules.insert(rule.id.as_str()) {
                return Err(ConfigError::DuplicateRuleId(rule.id.clone()));
            }
            if rule.matcher.is_empty() {
                return Err(ConfigError::RuleMatchesEverything(rule.id.clone()));
            }
            self.require_pool(&rule.route_to, &format!("rule `{}`", rule.id))?;
        }

        // `HintKind` is a closed catalog of four, so a linear scan beats
        // demanding `Ord` from `protocol` for the sake of a set.
        let mut seen_hints: Vec<(HintKind, &str)> = Vec::new();
        for route in &self.hint_routes {
            let key = (route.kind, route.value.as_str());
            if seen_hints.contains(&key) {
                return Err(ConfigError::DuplicateHintRoute {
                    kind: route.kind,
                    value: route.value.clone(),
                });
            }
            seen_hints.push(key);
            self.require_pool(&route.route_to, "hint_routes")?;
        }

        if let Some(heuristic) = &self.heuristic {
            if heuristic.weights.tokens_per_point == 0 {
                return Err(ConfigError::TokensPerPointIsZero);
            }
            self.require_pool(&heuristic.above, "heuristic.above")?;
            self.require_pool(&heuristic.below, "heuristic.below")?;
            for pair in heuristic.bands.windows(2) {
                if pair[0].at_least <= pair[1].at_least {
                    return Err(ConfigError::BandsNotDescending {
                        first: pair[0].at_least,
                        second: pair[1].at_least,
                    });
                }
            }
            for band in &heuristic.bands {
                self.require_pool(&band.pool, "heuristic.bands")?;
            }
        }

        if let RecoveryPolicy::Ordered { pools } = &self.recovery {
            for pool in pools {
                self.require_pool(pool, "recovery.ordered")?;
            }
        }

        Ok(())
    }

    fn require_pool(&self, pool: &str, referenced_by: &str) -> Result<(), ConfigError> {
        if self.pools.contains_key(pool) {
            Ok(())
        } else {
            Err(ConfigError::UnknownPool {
                pool: pool.to_owned(),
                referenced_by: referenced_by.to_owned(),
            })
        }
    }
}

/// Why a configuration cannot be routed with.
///
/// Enumerable on purpose. A remote config service that pushes a broken profile
/// has to be told which line to fix, and the client has to log a reason it can
/// show without the profile in hand.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConfigError {
    UnsupportedVersion(u32),
    AssumedContextWindowIsZero,
    NoPools,
    EmptyPool(String),
    UnknownPool {
        pool: String,
        referenced_by: String,
    },
    EmptyRuleId,
    DuplicateRuleId(String),
    /// A rule with no predicates matches every request, and silently kills every
    /// rule below it.
    RuleMatchesEverything(String),
    DuplicateHintRoute {
        kind: HintKind,
        value: String,
    },
    TokensPerPointIsZero,
    /// Band order on disk is the precedence order; an ascending or equal pair
    /// means a band that can never fire.
    BandsNotDescending {
        first: u32,
        second: u32,
    },
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => write!(
                f,
                "当前配置版本是 {version}，但 Token Station 只支持版本 {CONFIG_VERSION}。请先更新或重新创建配置，然后再启动。"
            ),
            Self::AssumedContextWindowIsZero => f.write_str(
                "上下文窗口不能设为 0。请先把 `assumed_context_window` 设为大于 0 的值，然后再启动。",
            ),
            Self::NoPools => f.write_str(
                "你还没有设置路由池。必须先设置至少一个路由池，才能启动 Token Station。",
            ),
            Self::EmptyPool(pool) => write!(
                f,
                "路由池 `{pool}` 里还没有模型。请先添加至少一个供应商和模型，然后再启动。"
            ),
            Self::UnknownPool {
                pool,
                referenced_by,
            } => write!(
                f,
                "{referenced_by} 使用了不存在的路由池 `{pool}`。请创建这个路由池，或改选一个已有路由池。"
            ),
            Self::EmptyRuleId => f.write_str(
                "有一条路由规则没有 ID。请为每条规则设置唯一 ID，然后再保存。",
            ),
            Self::DuplicateRuleId(id) => write!(
                f,
                "多条路由规则使用了同一个 ID `{id}`。请为每条规则设置唯一 ID，然后再保存。"
            ),
            Self::RuleMatchesEverything(id) => write!(
                f,
                "路由规则 `{id}` 没有任何条件，会匹配所有请求。请为它添加条件，或改用 `default_pool` 作为兜底。"
            ),
            Self::DuplicateHintRoute { kind, value } => write!(
                f,
                "多条提示路由使用了相同条件 `{kind:?}` = `{value}`。请只保留其中一条。"
            ),
            Self::TokensPerPointIsZero => {
                f.write_str(
                    "路由评分参数 `heuristic.weights.tokens_per_point` 不能是 0。请把它设为大于 0 的值。",
                )
            }
            Self::BandsNotDescending { first, second } => write!(
                f,
                "路由分档必须按 `at_least` 从高到低排列。当前 {first} 后面跟着 {second}，请调整顺序。"
            ),
        }
    }
}

impl Error for ConfigError {}

#[cfg(test)]
mod tests {
    use super::{ConfigError, Heuristic, Match, RouterConfig, Weights};
    use crate::RequestFeatures;
    use crate::test_support::{config, upstream_model};
    use token_station_protocol::{ChatRequest, HintKind, Message, Role};

    #[test]
    fn ported_weights_score_and_simple_talk_subtracts_saturating_at_zero() {
        let heuristic = Heuristic {
            weights: Weights {
                tokens_per_point: 1000,
                per_tool: 0,
                json_schema: 0,
                image: 0,
                per_code_block: 0,
                per_extra_turn: 0,
                per_reasoning_marker: 6,
                per_technical_term: 4,
                per_simple_indicator: 4,
                per_code_keyword: 2,
                per_math_term: 3,
                per_creative_term: 2,
                per_multi_step_point: 1,
                per_question: 1,
                system_format: 3,
                max_output_tokens_per_point: 10_000,
            },
            threshold: 15,
            above: "sota".to_owned(),
            below: "cheap".to_owned(),
            bands: Vec::new(),
        };

        let features = RequestFeatures {
            reasoning_marker_count: 2,                  // 12
            technical_term_count: 1,                    // 4
            math_term_count: 1,                         // 3
            multi_step_signal: 6,                       // 6
            question_count: 2,                          // 2
            system_format_hint: true,                   // 3
            requested_max_output_tokens: Some(100_000), // 10
            simple_indicator_count: 3,                  // -12
            ..RequestFeatures::default()
        };
        assert_eq!(heuristic.score(&features), 12 + 4 + 3 + 6 + 2 + 3 + 10 - 12);

        // All-easy talk cannot go negative.
        let easy = RequestFeatures {
            simple_indicator_count: 10,
            ..RequestFeatures::default()
        };
        assert_eq!(heuristic.score(&easy), 0);
    }

    #[test]
    fn six_dimension_profiles_keep_their_exact_pre_port_score() {
        // A pre-0.2 weights document (six fields, no bands) must deserialize
        // and produce the same score it always did.
        let heuristic: Heuristic = serde_json::from_str(
            r#"{
                "weights": {
                    "tokens_per_point": 100, "per_tool": 20, "json_schema": 10,
                    "image": 15, "per_code_block": 8, "per_extra_turn": 3
                },
                "threshold": 40, "above": "sota", "below": "cheap"
            }"#,
        )
        .expect("a pre-0.2 heuristic document still parses");
        assert!(heuristic.bands.is_empty());

        let features = RequestFeatures {
            conversation_tokens: 2_000, // 20 from conversation tokens, excluding the system prompt
            tool_count: 1,              // Excluded from difficulty; used only for capability gating
            code_block_count: 1,        // 8
            message_count: 3,           // 6 from two extra turns times 3
            // Ported counts present but every ported weight defaulted to 0.
            reasoning_marker_count: 9,
            simple_indicator_count: 9,
            ..RequestFeatures::default()
        };
        assert_eq!(heuristic.score(&features), 34); // 20 + 8 + 6; tool_count no longer scores
        assert_eq!(heuristic.select(54), ("sota", 40));
        assert_eq!(heuristic.select(39), ("cheap", 40));
    }

    #[test]
    fn bands_pick_the_first_tier_the_score_reaches() {
        let mut config = config();
        config.pools.insert(
            "medium".to_owned(),
            vec![upstream_model("openai_personal", "gpt-5.4-mini")],
        );
        config.pools.insert(
            "reasoning".to_owned(),
            vec![upstream_model("anthropic_personal", "claude-opus-4-8")],
        );
        let heuristic = Heuristic {
            weights: base_weights(),
            threshold: 15,
            above: "sota".to_owned(),
            below: "cheap".to_owned(),
            bands: vec![
                super::Band {
                    at_least: 35,
                    pool: "reasoning".to_owned(),
                },
                super::Band {
                    at_least: 15,
                    pool: "sota".to_owned(),
                },
                super::Band {
                    at_least: 5,
                    pool: "medium".to_owned(),
                },
                super::Band {
                    at_least: 0,
                    pool: "cheap".to_owned(),
                },
            ],
        };
        config.heuristic = Some(heuristic.clone());
        config
            .validate()
            .expect("descending bands over known pools");

        assert_eq!(heuristic.select(40), ("reasoning", 35));
        assert_eq!(heuristic.select(15), ("sota", 15));
        assert_eq!(heuristic.select(7), ("medium", 5));
        assert_eq!(heuristic.select(0), ("cheap", 0));
    }

    #[test]
    fn ascending_or_equal_bands_and_unknown_band_pools_are_refused() {
        let mut config = config();
        config.heuristic = Some(Heuristic {
            weights: base_weights(),
            threshold: 15,
            above: "sota".to_owned(),
            below: "cheap".to_owned(),
            bands: vec![
                super::Band {
                    at_least: 10,
                    pool: "cheap".to_owned(),
                },
                super::Band {
                    at_least: 20,
                    pool: "sota".to_owned(),
                },
            ],
        });
        assert_eq!(
            config.validate(),
            Err(ConfigError::BandsNotDescending {
                first: 10,
                second: 20
            })
        );

        config.heuristic = Some(Heuristic {
            weights: base_weights(),
            threshold: 15,
            above: "sota".to_owned(),
            below: "cheap".to_owned(),
            bands: vec![super::Band {
                at_least: 0,
                pool: "gpu-farm".to_owned(),
            }],
        });
        assert_eq!(
            config.validate(),
            Err(ConfigError::UnknownPool {
                pool: "gpu-farm".to_owned(),
                referenced_by: "heuristic.bands".to_owned(),
            })
        );
    }

    #[test]
    fn a_rule_can_force_a_pool_on_reasoning_markers() {
        // V1's "2+ reasoning markers force the reasoning tier", now spelled
        // as an operator rule instead of a branch inside the score.
        let matcher = Match {
            reasoning_markers_at_least: Some(2),
            ..Match::default()
        };
        assert!(!matcher.is_empty());

        let features = RequestFeatures {
            reasoning_marker_count: 2,
            ..RequestFeatures::default()
        };
        let request = ChatRequest::new("auto", Vec::new());
        assert!(matcher.matches(&features, &request));

        let features = RequestFeatures {
            reasoning_marker_count: 1,
            ..RequestFeatures::default()
        };
        assert!(!matcher.matches(&features, &request));
    }

    fn base_weights() -> Weights {
        serde_json::from_str(
            r#"{
                "tokens_per_point": 100, "per_tool": 20, "json_schema": 10,
                "image": 15, "per_code_block": 8, "per_extra_turn": 3
            }"#,
        )
        .expect("six-field weights parse")
    }

    #[test]
    fn a_rule_with_no_predicates_is_refused() {
        let mut broken = config();
        broken.rules[0].matcher = Match::default();

        assert_eq!(
            broken.validate(),
            Err(ConfigError::RuleMatchesEverything(
                "long-context".to_owned()
            ))
        );
    }

    #[test]
    fn a_missing_routing_pool_explains_how_to_continue() {
        let mut broken = config();
        broken.pools.clear();

        let error = broken.validate().expect_err("missing pools must be rejected");
        assert_eq!(error, ConfigError::NoPools);
        assert_eq!(
            error.to_string(),
            "你还没有设置路由池。必须先设置至少一个路由池，才能启动 Token Station。"
        );
    }

    #[test]
    fn a_pool_nobody_defined_is_named_together_with_who_wanted_it() {
        let mut broken = config();
        broken.rules[0].route_to = "gpu-farm".to_owned();

        assert_eq!(
            broken.validate(),
            Err(ConfigError::UnknownPool {
                pool: "gpu-farm".to_owned(),
                referenced_by: "rule `long-context`".to_owned(),
            })
        );
    }

    #[test]
    fn a_zero_divisor_is_refused_before_it_can_panic() {
        let mut broken = config();
        broken
            .heuristic
            .as_mut()
            .expect("heuristic")
            .weights
            .tokens_per_point = 0;

        assert_eq!(broken.validate(), Err(ConfigError::TokensPerPointIsZero));
    }

    #[test]
    fn duplicate_rule_ids_are_refused_because_a_record_could_not_name_one() {
        let mut broken = config();
        let clone = broken.rules[0].clone();
        broken.rules.push(clone);

        assert_eq!(
            broken.validate(),
            Err(ConfigError::DuplicateRuleId("long-context".to_owned()))
        );
    }

    #[test]
    fn an_empty_pool_is_refused() {
        let mut broken = config();
        broken.pools.insert("cheap".to_owned(), Vec::new());

        assert_eq!(
            broken.validate(),
            Err(ConfigError::EmptyPool("cheap".to_owned()))
        );
    }

    #[test]
    fn a_misspelled_predicate_fails_to_parse_rather_than_matching_everything() {
        let raw = r#"{"id":"tools","when":{"requires_tols":true},"route_to":"sota"}"#;
        let parsed: Result<super::Rule, _> = serde_json::from_str(raw);

        let message = parsed.expect_err("a typo must not deserialize").to_string();
        assert!(message.contains("requires_tols"), "{message}");
    }

    #[test]
    fn version_is_checked_before_anything_it_would_reinterpret() {
        let mut broken = config();
        broken.version = 2;
        broken.pools.clear();

        assert_eq!(broken.validate(), Err(ConfigError::UnsupportedVersion(2)));
    }

    #[test]
    fn an_unknown_default_pool_is_refused() {
        let mut broken = config();
        broken.default_pool = "nowhere".to_owned();

        assert!(matches!(
            broken.validate(),
            Err(ConfigError::UnknownPool { .. })
        ));
    }

    #[test]
    fn duplicate_hint_routes_are_refused() {
        let mut broken = config();
        let clone = broken.hint_routes[0].clone();
        broken.hint_routes.push(clone);

        assert_eq!(
            broken.validate(),
            Err(ConfigError::DuplicateHintRoute {
                kind: HintKind::StepType,
                value: "planning".to_owned(),
            })
        );
    }

    #[test]
    fn keywords_match_case_insensitively_and_across_message_parts() {
        let matcher = Match {
            keywords_any: vec!["PROOF".to_owned(), "推导".to_owned()],
            ..Match::default()
        };
        let features = RequestFeatures::default();

        let hit = ChatRequest::new(
            "auto",
            vec![Message::text(Role::User, "please write a proof sketch")],
        );
        let cjk = ChatRequest::new("auto", vec![Message::text(Role::User, "帮我做个推导")]);
        let miss = ChatRequest::new(
            "auto",
            vec![Message::text(Role::User, "what is the weather")],
        );

        assert!(matcher.matches(&features, &hit));
        assert!(matcher.matches(&features, &cjk));
        assert!(!matcher.matches(&features, &miss));
    }

    #[test]
    fn predicates_are_conjunctive() {
        let matcher = Match {
            requires_tools: Some(true),
            estimated_input_tokens_at_least: Some(100),
            ..Match::default()
        };
        let request = ChatRequest::new("auto", Vec::new());

        let both = RequestFeatures {
            tool_count: 1,
            estimated_input_tokens: 100,
            ..RequestFeatures::default()
        };
        let only_tools = RequestFeatures {
            tool_count: 1,
            ..RequestFeatures::default()
        };

        assert!(matcher.matches(&both, &request));
        assert!(!matcher.matches(&only_tools, &request));
    }

    #[test]
    fn requires_tools_false_demands_their_absence() {
        let matcher = Match {
            requires_tools: Some(false),
            ..Match::default()
        };
        let request = ChatRequest::new("auto", Vec::new());

        assert!(matcher.matches(&RequestFeatures::default(), &request));
        assert!(!matcher.matches(
            &RequestFeatures {
                tool_count: 2,
                ..RequestFeatures::default()
            },
            &request
        ));
    }

    #[test]
    fn the_score_saturates_rather_than_wrapping() {
        let heuristic = Heuristic {
            weights: Weights {
                tokens_per_point: 1,
                per_tool: u32::MAX,
                json_schema: 5,
                image: 5,
                per_code_block: 5,
                per_extra_turn: 5,
                per_reasoning_marker: 0,
                per_technical_term: 0,
                per_simple_indicator: 0,
                per_code_keyword: 0,
                per_math_term: 0,
                per_creative_term: 0,
                per_multi_step_point: 0,
                per_question: 0,
                system_format: 0,
                max_output_tokens_per_point: 0,
            },
            threshold: 10,
            above: "sota".to_owned(),
            below: "cheap".to_owned(),
            bands: Vec::new(),
        };
        let features = RequestFeatures {
            // tokens_per_point=1 makes the base u32::MAX; json_schema then exercises saturating_add.
            conversation_tokens: u32::MAX,
            requires_json_schema: true,
            ..RequestFeatures::default()
        };

        assert_eq!(heuristic.score(&features), u32::MAX);
    }

    #[test]
    fn config_round_trips_through_json() {
        let original = config();
        let encoded = serde_json::to_string(&original).expect("serializable config");
        let decoded: RouterConfig = serde_json::from_str(&encoded).expect("valid config");

        assert_eq!(decoded, original);
    }

    #[test]
    fn pool_members_keep_the_order_the_operator_wrote() {
        let config = config();
        let sota = &config.pools["sota"];

        assert_eq!(sota[0], upstream_model("openai_personal", "gpt-5.5"));
        assert_eq!(
            sota[1],
            upstream_model("anthropic_personal", "claude-opus-4-8")
        );
    }
}
