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
        let total = per(usage.input_tokens, self.input_per_mtok)
            .saturating_add(per(usage.output_tokens, self.output_per_mtok))
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
    /// The cost of `usage` under `model`, and the table version that priced it.
    /// `None` when the model has no entry — an unknown cost, never a zero one.
    #[must_use]
    pub fn price(&self, model: &str, usage: &Usage) -> Option<(i64, u32)> {
        self.models
            .get(model)
            .map(|price| (price.cost_micros(usage), self.version))
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

fn validate_model_id(model: &str) -> Result<(), String> {
    if model.is_empty()
        || model.len() > 256
        || model.trim() != model
        || model.chars().any(char::is_control)
    {
        return Err("pricing model id must be 1-256 trimmed, non-control characters".to_string());
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
}
