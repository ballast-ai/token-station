//! Versioned pricing: turn a request's token [`Usage`] into a cost, and record
//! *which* price table did it.
//!
//! Prices change. If a request's cost were recomputed from today's table, a
//! historical bill would silently become a different number than the one the
//! provider actually charged. So the table carries a `version`, the cost is
//! computed once at settle time, and the version is stored next to the cost —
//! a later price edit re-values nothing that already happened.
//!
//! A model with no entry has an *unknown* cost (`None`), never a zero one:
//! zero would quietly claim a request was free.

use std::collections::BTreeMap;

use token_station_protocol::Usage;

const MAX_PRICE_PER_MTOK: u64 = 9_000_000_000_000_000;
// A provider-scoped catalog key is `<upstream>/<model>`. Model discovery accepts
// 512-byte supplier IDs and a validated upstream reference has no smaller core
// limit, so leave bounded headroom for the scope while still rejecting
// unbounded operator input.
const MAX_PRICING_MODEL_ID_BYTES: usize = 1_024;

/// Micro-units of currency per one million tokens, per token class. `0` is a
/// real price (a free tier), distinct from a model that is simply absent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPrice {
    #[serde(default)]
    pub input_per_mtok: u64,
    #[serde(default)]
    pub output_per_mtok: u64,
    #[serde(default)]
    pub cache_read_per_mtok: u64,
    #[serde(default)]
    pub cache_write_per_mtok: u64,
    /// Reasoning tokens are billed at the output rate unless a price is given.
    #[serde(default)]
    pub reasoning_per_mtok: Option<u64>,
}

impl ModelPrice {
    /// Rejects rates that cannot be represented exactly at the Desktop's JSON
    /// command boundary. Zero is intentionally valid: it is an explicit free
    /// price, unlike an absent model entry.
    ///
    /// # Errors
    ///
    /// Returns an error when any rate exceeds the safe configured ceiling.
    pub fn validate(&self) -> Result<(), String> {
        for (name, rate) in [
            ("input_per_mtok", Some(self.input_per_mtok)),
            ("output_per_mtok", Some(self.output_per_mtok)),
            ("cache_read_per_mtok", Some(self.cache_read_per_mtok)),
            ("cache_write_per_mtok", Some(self.cache_write_per_mtok)),
            ("reasoning_per_mtok", self.reasoning_per_mtok),
        ] {
            if rate.is_some_and(|rate| rate > MAX_PRICE_PER_MTOK) {
                return Err(format!(
                    "{name} must not exceed {MAX_PRICE_PER_MTOK} micro-units per million tokens"
                ));
            }
        }
        Ok(())
    }

    /// The cost of one exchange, in micro-units. Saturating throughout: a
    /// pathological token count caps rather than wraps.
    #[must_use]
    pub fn cost_micros(&self, usage: &Usage) -> i64 {
        let per = |tokens: u64, rate: u64| -> u128 {
            u128::from(tokens).saturating_mul(u128::from(rate)) / 1_000_000
        };
        let reasoning_rate = self.reasoning_per_mtok.unwrap_or(self.output_per_mtok);
        let fresh_input = usage
            .input_tokens
            .saturating_sub(usage.cache_read_tokens)
            .saturating_sub(usage.cache_write_tokens);
        let fresh_output = usage.output_tokens.saturating_sub(usage.reasoning_tokens);
        let total = per(fresh_input, self.input_per_mtok)
            .saturating_add(per(fresh_output, self.output_per_mtok))
            .saturating_add(per(usage.cache_read_tokens, self.cache_read_per_mtok))
            .saturating_add(per(usage.cache_write_tokens, self.cache_write_per_mtok))
            .saturating_add(per(usage.reasoning_tokens, reasoning_rate));
        i64::try_from(total).unwrap_or(i64::MAX)
    }
}

/// A versioned price table: which models cost what, and the version stamped onto
/// every request it prices.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PriceTable {
    /// Bumped whenever a price changes. `0` is "no table configured".
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub models: BTreeMap<String, ModelPrice>,
}

impl PriceTable {
    /// Built-in public list prices for common models. Rates are micro-USD per
    /// one million tokens and intentionally form a versioned estimate, not a
    /// claim about a provider's final invoice.
    #[must_use]
    pub fn builtin() -> Self {
        let price = |input, output, cache_read, cache_write| ModelPrice {
            input_per_mtok: input,
            output_per_mtok: output,
            cache_read_per_mtok: cache_read,
            cache_write_per_mtok: cache_write,
            reasoning_per_mtok: None,
        };
        Self {
            version: 1,
            models: BTreeMap::from([
                (
                    "claude-opus-4-8".to_owned(),
                    price(5_000_000, 25_000_000, 500_000, 6_250_000),
                ),
                (
                    "claude-sonnet-4".to_owned(),
                    price(3_000_000, 15_000_000, 300_000, 3_750_000),
                ),
                (
                    "claude-sonnet-4-6".to_owned(),
                    price(3_000_000, 15_000_000, 300_000, 3_750_000),
                ),
                (
                    "deepseek-chat".to_owned(),
                    price(270_000, 1_100_000, 28_000, 0),
                ),
                (
                    "deepseek-reasoner".to_owned(),
                    price(550_000, 2_190_000, 140_000, 0),
                ),
                (
                    "deepseek-v4-flash".to_owned(),
                    price(140_000, 280_000, 2_800, 0),
                ),
                (
                    "deepseek-v4-pro".to_owned(),
                    price(435_000, 870_000, 3_625, 0),
                ),
                ("glm-5".to_owned(), price(1_000_000, 3_200_000, 200_000, 0)),
                (
                    "gpt-5.5".to_owned(),
                    price(5_000_000, 30_000_000, 500_000, 0),
                ),
                (
                    "minimax-m3".to_owned(),
                    price(600_000, 2_400_000, 120_000, 0),
                ),
            ]),
        }
    }

    /// The cost of `usage` under `model`, and the table version that priced it.
    /// `None` when the model has no entry — an unknown cost, never a zero one.
    #[must_use]
    pub fn price(&self, model: &str, usage: &Usage) -> Option<(i64, u32)> {
        self.model_price(model)
            .map(|price| (price.cost_micros(usage), self.version))
    }

    /// Prices one concrete upstream/model target. Provider-scoped catalog
    /// prices win; legacy model-only tables remain the compatibility fallback.
    #[must_use]
    pub fn price_for_upstream(
        &self,
        upstream: &str,
        model: &str,
        usage: &Usage,
    ) -> Option<(i64, u32)> {
        self.model_price_for_upstream(upstream, model)
            .map(|price| (price.cost_micros(usage), self.version))
    }

    /// Resolves the immutable configured rates for one model using the same
    /// normalization rules as settlement. Agent connectors use this to expose
    /// an exact static price only when every reachable route agrees.
    #[must_use]
    pub fn model_price(&self, model: &str) -> Option<&ModelPrice> {
        let normalized = normalize_model_id(model);
        self.models
            .get(model)
            .or_else(|| self.models.get(&normalized))
            .or_else(|| {
                self.models
                    .iter()
                    .filter_map(|(candidate, price)| {
                        if candidate.contains('/') {
                            return None;
                        }
                        let candidate = normalize_model_id(candidate);
                        normalized
                            .strip_prefix(&candidate)
                            .filter(|suffix| suffix.starts_with('-'))
                            .map(|_| (candidate.len(), price))
                    })
                    .max_by_key(|(candidate_len, _)| *candidate_len)
                    .map(|(_, price)| price)
            })
    }

    #[must_use]
    pub fn model_price_for_upstream(&self, upstream: &str, model: &str) -> Option<&ModelPrice> {
        self.models
            .get(&format!("{upstream}/{model}"))
            .or_else(|| self.model_price(model))
    }

    /// Validates the current table without changing it.
    ///
    /// # Errors
    ///
    /// Rejects malformed model ids or unsafe numeric rates. Legacy manually
    /// authored tables may contain models at version zero; their first editor
    /// change upgrades them to version one instead of breaking startup.
    pub fn validate(&self) -> Result<(), String> {
        for (model, price) in &self.models {
            validate_model_id(model)?;
            price
                .validate()
                .map_err(|error| format!("model `{model}` price: {error}"))?;
        }
        Ok(())
    }

    /// Returns the next immutable price-table version with one model added or
    /// changed. Existing receipts and this table are untouched.
    ///
    /// # Errors
    ///
    /// Rejects invalid ids/rates, no-op edits, invalid current tables, and
    /// version exhaustion.
    pub fn next_with_model(&self, model: &str, price: ModelPrice) -> Result<Self, String> {
        self.validate()?;
        validate_model_id(model)?;
        price.validate()?;
        if self.models.get(model) == Some(&price) {
            return Err(format!("model `{model}` price is unchanged"));
        }
        let version = self
            .version
            .checked_add(1)
            .ok_or_else(|| "pricing version exhausted; start a new configuration".to_string())?;
        let mut models = self.models.clone();
        models.insert(model.to_string(), price);
        Ok(Self { version, models })
    }

    /// Returns the next immutable price-table version with several models
    /// added or changed as one atomic catalog import.
    ///
    /// # Errors
    ///
    /// Rejects an empty batch, invalid ids/rates, a batch that changes
    /// nothing, an invalid current table, and version exhaustion.
    pub fn next_with_models(
        &self,
        additions: BTreeMap<String, ModelPrice>,
    ) -> Result<Self, String> {
        self.validate()?;
        if additions.is_empty() {
            return Err("price batch must contain at least one model".to_owned());
        }
        for (model, price) in &additions {
            validate_model_id(model)?;
            price
                .validate()
                .map_err(|error| format!("model `{model}` price: {error}"))?;
        }
        if additions
            .iter()
            .all(|(model, price)| self.models.get(model) == Some(price))
        {
            return Err("model prices are unchanged".to_owned());
        }
        let version = self
            .version
            .checked_add(1)
            .ok_or_else(|| "pricing version exhausted; start a new configuration".to_owned())?;
        let mut models = self.models.clone();
        models.extend(additions);
        Ok(Self { version, models })
    }

    /// Returns the next immutable price-table version without one model.
    ///
    /// # Errors
    ///
    /// Rejects invalid current tables, missing models, and version exhaustion.
    pub fn next_without_model(&self, model: &str) -> Result<Self, String> {
        self.validate()?;
        validate_model_id(model)?;
        if !self.models.contains_key(model) {
            return Err(format!("model `{model}` has no configured price"));
        }
        let version = self
            .version
            .checked_add(1)
            .ok_or_else(|| "pricing version exhausted; start a new configuration".to_string())?;
        let mut models = self.models.clone();
        models.remove(model);
        Ok(Self { version, models })
    }
}

fn normalize_model_id(model: &str) -> String {
    let mut normalized = model
        .rsplit_once('/')
        .map_or(model, |(_, tail)| tail)
        .split(':')
        .next()
        .unwrap_or(model)
        .trim()
        .replace('@', "-")
        .to_ascii_lowercase();
    if normalized.starts_with("claude-") {
        normalized = normalized.replace('.', "-");
    }
    normalized
}

fn validate_model_id(model: &str) -> Result<(), String> {
    if model.is_empty()
        || model.len() > MAX_PRICING_MODEL_ID_BYTES
        || model.trim() != model
        || model.chars().any(char::is_control)
    {
        return Err(format!(
            "pricing model id must be 1-{MAX_PRICING_MODEL_ID_BYTES} bytes, trimmed, and non-control"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{ModelPrice, PriceTable};
    use std::collections::BTreeMap;
    use token_station_protocol::Usage;

    fn usage(input: u64, output: u64) -> Usage {
        Usage {
            input_tokens: input,
            output_tokens: output,
            ..Usage::default()
        }
    }

    #[test]
    fn cost_is_micro_units_per_million_tokens() {
        // $3 / Mtok input, $15 / Mtok output, in micro-dollars.
        let price = ModelPrice {
            input_per_mtok: 3_000_000,
            output_per_mtok: 15_000_000,
            ..ModelPrice::default()
        };
        // 1M input + 1M output = 3_000_000 + 15_000_000 micro-units.
        assert_eq!(price.cost_micros(&usage(1_000_000, 1_000_000)), 18_000_000);
    }

    #[test]
    fn cache_and_reasoning_subsets_are_not_billed_twice() {
        let price = ModelPrice {
            input_per_mtok: 1_000_000,
            output_per_mtok: 2_000_000,
            cache_read_per_mtok: 100_000,
            cache_write_per_mtok: 1_250_000,
            reasoning_per_mtok: None,
        };
        let value = Usage {
            input_tokens: 100,
            output_tokens: 20,
            cache_read_tokens: 60,
            cache_write_tokens: 20,
            reasoning_tokens: 15,
            ..Usage::default()
        };

        assert_eq!(price.cost_micros(&value), 91);
    }

    #[test]
    fn provider_namespaced_models_match_the_normalized_builtin_price() {
        let table = PriceTable {
            version: 3,
            models: BTreeMap::from([(
                "claude-opus-4-8".to_owned(),
                ModelPrice {
                    input_per_mtok: 5_000_000,
                    ..ModelPrice::default()
                },
            )]),
        };

        assert_eq!(
            table.price("anthropic/Claude-Opus-4.8", &usage(1_000_000, 0)),
            Some((5_000_000, 3)),
        );
    }

    #[test]
    fn provider_scoped_price_wins_and_model_only_price_remains_the_fallback() {
        let scoped = ModelPrice {
            input_per_mtok: 200_000,
            ..ModelPrice::default()
        };
        let fallback = ModelPrice {
            input_per_mtok: 900_000,
            ..ModelPrice::default()
        };
        let table = PriceTable {
            version: 8,
            models: BTreeMap::from([
                ("glm-5.2".to_owned(), fallback),
                ("wecoding/glm-5.2".to_owned(), scoped),
            ]),
        };

        assert_eq!(
            table.model_price_for_upstream("wecoding", "glm-5.2"),
            Some(&scoped)
        );
        assert_eq!(
            table.model_price_for_upstream("another", "glm-5.2"),
            Some(&fallback)
        );
        assert_eq!(
            table.price_for_upstream("wecoding", "glm-5.2", &usage(1_000_000, 0)),
            Some((200_000, 8))
        );
    }

    #[test]
    fn another_providers_scoped_price_never_becomes_a_model_only_fallback() {
        let scoped = ModelPrice {
            input_per_mtok: 200_000,
            ..ModelPrice::default()
        };
        let table = PriceTable {
            version: 9,
            models: BTreeMap::from([("wecoding/glm-5.2".to_owned(), scoped)]),
        };

        assert_eq!(
            table.model_price_for_upstream("another", "glm-5.2-variant"),
            None
        );
    }

    #[test]
    fn dated_and_reasoning_variant_suffixes_use_the_longest_matching_base_model() {
        let table = PriceTable {
            version: 4,
            models: BTreeMap::from([
                (
                    "claude-sonnet-4".to_owned(),
                    ModelPrice {
                        input_per_mtok: 3_000_000,
                        ..ModelPrice::default()
                    },
                ),
                (
                    "claude-sonnet-4-6".to_owned(),
                    ModelPrice {
                        input_per_mtok: 4_000_000,
                        ..ModelPrice::default()
                    },
                ),
            ]),
        };

        assert_eq!(
            table.price(
                "anthropic/claude-sonnet-4.6-20260217-thinking",
                &usage(1_000_000, 0),
            ),
            Some((4_000_000, 4)),
        );
    }

    #[test]
    fn builtin_catalog_prices_current_claude_deepseek_openai_minimax_and_glm_models() {
        let table = PriceTable::builtin();

        assert_eq!(table.version, 1);
        assert_eq!(
            table.price("deepseek-v4-pro", &usage(1_000_000, 0)),
            Some((435_000, 1)),
        );
        assert!(table.models.contains_key("claude-opus-4-8"));
        assert!(table.models.contains_key("gpt-5.5"));
        assert!(table.models.contains_key("minimax-m3"));
        assert!(table.models.contains_key("glm-5"));
    }

    #[test]
    fn an_unpriced_model_costs_unknown_not_zero() {
        let table = PriceTable {
            version: 7,
            models: BTreeMap::from([("gpt-5".to_owned(), ModelPrice::default())]),
        };
        assert_eq!(table.price("gpt-5", &usage(10, 10)), Some((0, 7)));
        assert_eq!(
            table.price("some-unpriced-model", &usage(10, 10)),
            None,
            "absent is unknown, not free"
        );
    }

    #[test]
    fn the_priced_version_travels_with_the_cost() {
        let mut table = PriceTable {
            version: 1,
            models: BTreeMap::from([(
                "m".to_owned(),
                ModelPrice {
                    output_per_mtok: 1_000_000,
                    ..ModelPrice::default()
                },
            )]),
        };
        let (_, v1) = table.price("m", &usage(0, 1_000_000)).unwrap();
        assert_eq!(v1, 1);

        // A later price change bumps the version; already-priced requests keep
        // theirs (the caller stores it), so history is not re-valued.
        table.version = 2;
        table.models.get_mut("m").unwrap().output_per_mtok = 2_000_000;
        let (cost, v2) = table.price("m", &usage(0, 1_000_000)).unwrap();
        assert_eq!((cost, v2), (2_000_000, 2));
    }

    #[test]
    fn editing_one_model_returns_a_new_table_version_and_leaves_the_old_table_unchanged() {
        let original = PriceTable {
            version: 7,
            models: BTreeMap::from([(
                "model-a".to_owned(),
                ModelPrice {
                    input_per_mtok: 1_000_000,
                    output_per_mtok: 2_000_000,
                    ..ModelPrice::default()
                },
            )]),
        };
        let replacement = ModelPrice {
            input_per_mtok: 3_000_000,
            output_per_mtok: 4_000_000,
            cache_read_per_mtok: 500_000,
            cache_write_per_mtok: 6_000_000,
            reasoning_per_mtok: Some(5_000_000),
        };

        let next = original
            .next_with_model("model-a", replacement)
            .expect("a changed price creates a new table");

        assert_eq!(next.version, 8);
        assert_eq!(next.models["model-a"], replacement);
        assert_eq!(original.version, 7);
        assert_eq!(original.models["model-a"].input_per_mtok, 1_000_000);
        assert!(
            original
                .next_with_model("model-a", original.models["model-a"])
                .is_err()
        );
    }

    #[test]
    fn adding_several_models_creates_one_price_table_version() {
        let original = PriceTable {
            version: 7,
            models: BTreeMap::from([(
                "existing/model".to_owned(),
                ModelPrice {
                    input_per_mtok: 900_000,
                    ..ModelPrice::default()
                },
            )]),
        };
        let additions = BTreeMap::from([
            (
                "deepseek/deepseek-v4-flash".to_owned(),
                ModelPrice {
                    input_per_mtok: 140_000,
                    output_per_mtok: 280_000,
                    ..ModelPrice::default()
                },
            ),
            (
                "deepseek/deepseek-v4-pro".to_owned(),
                ModelPrice {
                    input_per_mtok: 435_000,
                    output_per_mtok: 870_000,
                    ..ModelPrice::default()
                },
            ),
        ]);

        let next = original
            .next_with_models(additions)
            .expect("one batch creates one immutable version");

        assert_eq!(next.version, 8);
        assert_eq!(next.models.len(), 3);
        assert_eq!(original.version, 7);
        assert_eq!(original.models.len(), 1);
    }

    #[test]
    fn pricing_edits_validate_model_rates_deletion_and_version_exhaustion() {
        let empty = PriceTable::default();
        let free = empty
            .next_with_model("free/model", ModelPrice::default())
            .expect("an explicit all-zero price is valid");
        assert_eq!(free.version, 1);
        assert_eq!(free.price("free/model", &usage(1, 1)), Some((0, 1)));
        assert!(
            free.next_with_model(" free/model", ModelPrice::default())
                .is_err()
        );
        assert!(free.next_without_model("missing").is_err());
        let removed = free.next_without_model("free/model").unwrap();
        assert_eq!(removed.version, 2);
        assert!(removed.models.is_empty());

        let legacy_v0 = PriceTable {
            version: 0,
            models: BTreeMap::from([("legacy".to_owned(), ModelPrice::default())]),
        };
        assert_eq!(
            legacy_v0
                .next_with_model(
                    "legacy",
                    ModelPrice {
                        input_per_mtok: 1,
                        ..ModelPrice::default()
                    }
                )
                .unwrap()
                .version,
            1
        );

        let exhausted = PriceTable {
            version: u32::MAX,
            models: BTreeMap::new(),
        };
        assert!(
            exhausted
                .next_with_model("m", ModelPrice::default())
                .is_err()
        );
    }

    #[test]
    fn provider_scoped_catalog_keys_cover_the_desktop_identity_bounds() {
        let provider = "p".repeat(511);
        let model = "m".repeat(512);
        let scoped = format!("{provider}/{model}");

        let table = PriceTable::default()
            .next_with_model(&scoped, ModelPrice::default())
            .expect("a valid provider plus supplier model identity remains priceable");

        assert!(table.models.contains_key(&scoped));
        assert!(
            table
                .next_with_model(&format!("{scoped}xx"), ModelPrice::default())
                .is_err()
        );
    }
}
