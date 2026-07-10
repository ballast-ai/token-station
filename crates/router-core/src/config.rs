use std::collections::{BTreeMap, BTreeSet};
use std::error::Error;
use std::fmt;

use serde::{Deserialize, Serialize};
use token_station_protocol::{ChatRequest, Content, ContentPart, HintKind};

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
    /// preference; the router only reorders by health.
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
    /// Where a request goes when nothing above decided.
    pub default_pool: String,
    #[serde(default = "assumed_context_window_default")]
    pub assumed_context_window: u32,
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
                    ContentPart::ImageUrl { .. } => None,
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
}

impl Heuristic {
    /// The complexity score, saturating rather than wrapping.
    ///
    /// A request that overflows a `u32` of score is, for every purpose the
    /// threshold has, maximally complex.
    #[must_use]
    pub fn score(&self, features: &RequestFeatures) -> u32 {
        let weights = &self.weights;
        let mut score = features
            .estimated_input_tokens
            .saturating_div(weights.tokens_per_point.max(1));

        score = score.saturating_add(features.tool_count.saturating_mul(weights.per_tool));
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
        score
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
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedVersion(version) => write!(
                f,
                "config version {version} is not {CONFIG_VERSION}; a breaking change ships a new version rather than reinterpreting this one"
            ),
            Self::AssumedContextWindowIsZero => f.write_str(
                "assumed_context_window of 0 would make every model with an unreported context window unroutable",
            ),
            Self::NoPools => f.write_str("a router with no pools can route nothing"),
            Self::EmptyPool(pool) => write!(f, "pool `{pool}` has no members"),
            Self::UnknownPool {
                pool,
                referenced_by,
            } => write!(f, "{referenced_by} routes to pool `{pool}`, which does not exist"),
            Self::EmptyRuleId => f.write_str(
                "a rule must have an id; it is what the decision record and the audit log name",
            ),
            Self::DuplicateRuleId(id) => write!(
                f,
                "two rules share the id `{id}`; a decision record could not say which one fired"
            ),
            Self::RuleMatchesEverything(id) => write!(
                f,
                "rule `{id}` has no predicates, so it matches every request and every rule after it is dead; use `default_pool`"
            ),
            Self::DuplicateHintRoute { kind, value } => write!(
                f,
                "two hint routes share `{kind:?}` = `{value}`"
            ),
            Self::TokensPerPointIsZero => {
                f.write_str("heuristic.weights.tokens_per_point of 0 would divide by zero")
            }
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
            },
            threshold: 10,
            above: "sota".to_owned(),
            below: "cheap".to_owned(),
        };
        let features = RequestFeatures {
            estimated_input_tokens: 1_000,
            tool_count: 2,
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
