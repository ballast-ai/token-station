//! Accounting across provider attempts, including retries and host-owned tool loops.

use super::{CostKind, RequestRecord, Usage, UsageObservation, can_estimate};
use crate::pricing::PriceTable;
use token_station_router_core::UpstreamModel;

#[derive(Default)]
pub(crate) struct Aggregate {
    usage: Option<Usage>,
    observation: Option<UsageObservation>,
    cost: Option<i64>,
    estimated: bool,
    price_version: Option<u32>,
    attempts: u32,
    first_target: Option<UpstreamModel>,
    different_targets: bool,
    actual_charge: bool,
}

impl Aggregate {
    pub(crate) fn absorb(
        &mut self,
        record: &RequestRecord,
        status: Option<u16>,
        target: &UpstreamModel,
        pricing: &PriceTable,
    ) {
        // A rejected request without usage is not a generation round. Unknown
        // transport outcomes remain unknown instead of becoming a free round.
        if status.is_some_and(|status| status >= 400)
            && record.usage.is_none()
            && record.cost_micros.is_none()
        {
            return;
        }
        if let Some(first) = &self.first_target {
            self.different_targets |= first != target;
        } else {
            self.first_target = Some(target.clone());
        }
        self.actual_charge |= record.cost_kind == CostKind::Actual && record.cost_micros.is_some();
        let mut observation = record.usage_observation.unwrap_or_default();
        observation.incomplete |=
            observation.input_tokens.is_none() || observation.output_tokens.is_none();
        if let Some(usage) = record.usage {
            let sum = self.usage.get_or_insert_with(Usage::default);
            macro_rules! add {
                ($($field:ident),+ $(,)?) => {$(
                    match sum.$field.checked_add(usage.$field) {
                        Some(value) => sum.$field = value,
                        None => {
                            sum.$field = u64::MAX;
                            observation.incomplete = true;
                        }
                    }
                )+};
            }
            add!(
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
                cache_write_5m_tokens,
                cache_write_1h_tokens,
                reasoning_tokens
            );
        }
        if let Some(sum) = &mut self.observation {
            sum.incomplete |= observation.incomplete;
            macro_rules! add {
                ($($field:ident),+ $(,)?) => {$(
                    sum.$field = sum.$field.zip(observation.$field)
                        .and_then(|(left, right)| left.checked_add(right));
                )+};
            }
            add!(
                input_tokens,
                output_tokens,
                cache_read_tokens,
                cache_write_tokens,
                cache_write_5m_tokens,
                cache_write_1h_tokens,
                reasoning_tokens
            );
            sum.incomplete |= sum.input_tokens.is_none() || sum.output_tokens.is_none();
        } else {
            self.observation = Some(observation);
        }

        let price = if record.cost_kind == CostKind::Actual {
            record.cost_micros.map(|cost| (cost, None))
        } else {
            record
                .usage
                .filter(|usage| can_estimate(usage, record.usage_observation))
                .and_then(|usage| {
                    pricing.price_for_upstream(target.upstream.as_str(), &target.model, &usage)
                })
                .map(|(cost, version)| (cost, Some(version)))
        };
        if let Some((_, Some(version))) = price {
            self.estimated = true;
            self.price_version = Some(version);
        }
        self.cost = if self.attempts == 0 {
            price.map(|(cost, _)| cost)
        } else {
            self.cost
                .zip(price)
                .and_then(|(sum, (cost, _))| sum.checked_add(cost))
        };
        self.attempts += 1;
    }

    pub(crate) fn apply(&self, record: &mut RequestRecord) {
        record.usage = self.usage;
        record.usage_observation = self.observation.map(|mut observation| {
            observation.cost_requires_attempt_pricing = self.cost.is_none()
                && self.attempts > 1
                && (self.different_targets || self.actual_charge);
            observation
        });
        record.cost_micros = self.cost;
        record.cost_kind = match (self.cost, self.estimated) {
            (None, _) => CostKind::Unknown,
            (Some(_), true) => CostKind::Estimated,
            (Some(_), false) => CostKind::Actual,
        };
        record.price_version = if self.cost.is_some() && self.estimated {
            self.price_version
        } else {
            None
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::accounting::AccountingTap;
    use token_station_metrics::Recorder;
    use token_station_router_core::UpstreamRef;

    fn attempt(endpoint: &str, body: &str) -> RequestRecord {
        let mut tap = AccountingTap::new(endpoint);
        tap.head(200, "application/json");
        tap.push(body.as_bytes());
        let mut record = RequestRecord::begin(1, "openai-responses");
        tap.finish(&mut record);
        record
    }

    fn target(upstream: &str) -> UpstreamModel {
        UpstreamModel::new(UpstreamRef::new(upstream).unwrap(), "model")
    }

    fn prices() -> PriceTable {
        serde_json::from_value(serde_json::json!({
            "version":77,"models":{
                "first/model":{"input_per_mtok":1_000_000,"output_per_mtok":1_000_000},
                "second/model":{"input_per_mtok":2_000_000,"output_per_mtok":2_000_000}
            }
        }))
        .unwrap()
    }

    #[test]
    fn actual_round_costs_and_usage_survive_database_and_statistics() {
        let mut aggregate = Aggregate::default();
        for cost in ["0.01", "0.02"] {
            let record = attempt(
                "https://openrouter.ai/api/v1",
                &format!(
                    r#"{{"usage":{{"prompt_tokens":10,"completion_tokens":5,"cost":{cost}}}}}"#
                ),
            );
            aggregate.absorb(&record, Some(200), &target("first"), &prices());
        }
        let mut record = RequestRecord::begin(1, "openai-responses");
        record.request_id = "aggregate".into();
        record.status = 200;
        aggregate.apply(&mut record);
        assert_eq!(record.cost_kind, CostKind::Actual);
        assert_eq!(record.cost_micros, Some(30_000));
        let directory =
            std::env::temp_dir().join(format!("ts-search-actual-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let database = directory.join("metrics.sqlite");
        let store = crate::store::SqliteStore::open(&database).unwrap();
        store.record(&record);
        let restored = crate::store::recent_receipts(&database, 1).unwrap();
        assert_eq!(restored[0].usage.unwrap().input_tokens, 20);
        assert_eq!(restored[0].usage.unwrap().output_tokens, 10);
        assert_eq!(restored[0].cost_micros, Some(30_000));
        let report = crate::stats::collect(&database, None, None).unwrap();
        assert_eq!(report.total.cost_micros, Some(30_000));
        assert_eq!(report.total.actual_cost_requests, 1);
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn mixed_providers_are_priced_before_aggregation() {
        let mut aggregate = Aggregate::default();
        let actual = attempt(
            "https://openrouter.ai/api/v1",
            r#"{"usage":{"prompt_tokens":10,"completion_tokens":5,"cost":0.01}}"#,
        );
        let estimated = attempt(
            "https://example.test",
            r#"{"usage":{"prompt_tokens":10,"completion_tokens":5}}"#,
        );
        aggregate.absorb(&actual, Some(200), &target("first"), &prices());
        aggregate.absorb(&estimated, Some(200), &target("second"), &prices());
        let mut record = RequestRecord::begin(0, "openai-responses");
        aggregate.apply(&mut record);
        assert_eq!(record.cost_kind, CostKind::Estimated);
        assert_eq!(record.cost_micros, Some(10_030));
        assert_eq!(record.price_version, Some(77));
    }

    #[test]
    fn an_unpriced_round_never_becomes_free_or_uses_the_final_model_price() {
        let mut aggregate = Aggregate::default();
        let usage = attempt(
            "https://example.test",
            r#"{"usage":{"prompt_tokens":10,"completion_tokens":5}}"#,
        );
        aggregate.absorb(&usage, Some(200), &target("unpriced"), &prices());
        aggregate.absorb(&usage, Some(200), &target("second"), &prices());
        let mut record = RequestRecord::begin(0, "openai-responses");
        aggregate.apply(&mut record);
        assert_eq!(record.cost_kind, CostKind::Unknown);
        assert_eq!(record.cost_micros, None);
        assert_eq!(record.price_version, None);
        assert_eq!(record.usage_observation.unwrap().input_tokens, Some(20));
        assert!(!record.usage_observation.unwrap().incomplete);
    }

    #[test]
    fn failed_attempts_preserve_reported_charges_and_unknown_usage() {
        let mut aggregate = Aggregate::default();
        let mut partial = attempt(
            "https://openrouter.ai/api/v1",
            r#"{"usage":{"prompt_tokens":10,"cost":0.01}}"#,
        );
        partial.usage_observation.as_mut().unwrap().incomplete = true;
        aggregate.absorb(&partial, Some(200), &target("first"), &prices());
        let complete = attempt(
            "https://openrouter.ai/api/v1",
            r#"{"usage":{"prompt_tokens":20,"completion_tokens":5,"cost":0.02}}"#,
        );
        aggregate.absorb(&complete, Some(200), &target("second"), &prices());
        let mut record = RequestRecord::begin(0, "openai-responses");
        aggregate.apply(&mut record);
        assert_eq!(record.cost_kind, CostKind::Actual);
        assert_eq!(record.cost_micros, Some(30_000));
        assert_eq!(record.usage_observation.unwrap().input_tokens, Some(30));
        assert_eq!(record.usage_observation.unwrap().output_tokens, None);
        assert!(record.usage_observation.unwrap().incomplete);
        assert_eq!(record.usage.unwrap().output_tokens, 5);
    }

    #[test]
    fn explicit_rejections_do_not_erase_known_rounds_but_transport_unknowns_do() {
        let mut aggregate = Aggregate::default();
        let complete = attempt(
            "https://openrouter.ai/api/v1",
            r#"{"usage":{"prompt_tokens":10,"completion_tokens":5,"cost":0.01}}"#,
        );
        let empty = RequestRecord::begin(0, "openai-responses");
        aggregate.absorb(&empty, Some(400), &target("first"), &prices());
        aggregate.absorb(&complete, Some(200), &target("first"), &prices());
        let mut record = RequestRecord::begin(0, "openai-responses");
        aggregate.apply(&mut record);
        assert_eq!(record.cost_micros, Some(10_000));
        aggregate.absorb(&empty, None, &target("second"), &prices());
        aggregate.apply(&mut record);
        assert_eq!(record.cost_micros, None);
        assert!(record.usage_observation.unwrap().incomplete);
    }
    #[test]
    fn backfill_preserves_same_target_loops_and_retries_without_repricing_mixed_scopes() {
        let directory =
            std::env::temp_dir().join(format!("ts-search-backfill-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let database = directory.join("metrics.sqlite");
        let store = crate::store::SqliteStore::open(&database).unwrap();
        let usage = attempt(
            "https://example.test",
            r#"{"usage":{"prompt_tokens":10,"completion_tokens":5}}"#,
        );
        for scenario in [
            "same_target",
            "mixed_targets",
            "ordinary_retry",
            "actual_then_unpriced",
        ] {
            let mut aggregate = Aggregate::default();
            let mut record = RequestRecord::begin(1, "openai-responses");
            record.request_id = scenario.to_owned();
            record.status = 200;
            record.attempts = 2;
            record.routing = Some(token_station_metrics::RoutingRecord {
                upstream: "second".into(),
                model: "model".into(),
                pool: "main".into(),
                decided_by: token_station_metrics::RecordedDecidedBy::Default,
                fallbacks: 0,
                features: token_station_router_core::RequestFeatures::default(),
            });
            if scenario == "ordinary_retry" {
                record.usage = usage.usage;
                record.usage_observation = usage.usage_observation;
            } else {
                let first = if scenario == "actual_then_unpriced" {
                    attempt(
                        "https://openrouter.ai/api/v1",
                        r#"{"usage":{"prompt_tokens":10,"completion_tokens":5,"cost":0.01}}"#,
                    )
                } else {
                    usage.clone()
                };
                aggregate.absorb(
                    &first,
                    Some(200),
                    &target(if scenario == "mixed_targets" {
                        "first"
                    } else {
                        "second"
                    }),
                    &PriceTable::default(),
                );
                aggregate.absorb(&usage, Some(200), &target("second"), &PriceTable::default());
                aggregate.apply(&mut record);
            }
            store.record(&record);
        }
        assert_eq!(
            crate::store::SqliteStore::backfill_unknown_costs(&database, &prices()).unwrap(),
            2
        );
        let restored = crate::store::recent_receipts(&database, 10).unwrap();
        for record in restored {
            match record.request_id.as_str() {
                "same_target" => assert_eq!(record.cost_micros, Some(60)),
                "ordinary_retry" => assert_eq!(record.cost_micros, Some(30)),
                _ => {
                    assert_eq!(record.cost_micros, None);
                    assert!(
                        record
                            .usage_observation
                            .unwrap()
                            .cost_requires_attempt_pricing
                    );
                }
            }
        }
        drop(store);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn legacy_observation_json_keeps_backfill_eligibility() {
        let value: UsageObservation =
            serde_json::from_str(r#"{"input_tokens":10,"output_tokens":5}"#).unwrap();
        assert!(!value.cost_requires_attempt_pricing);
        assert!(
            !serde_json::to_value(value)
                .unwrap()
                .as_object()
                .unwrap()
                .contains_key("cost_requires_attempt_pricing")
        );
    }
}
