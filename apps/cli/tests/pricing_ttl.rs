use token_station_cli::pricing::ModelPrice;
use token_station_protocol::Usage;

#[test]
fn ttl_prices_charge_each_cache_class_and_preserve_legacy_fallback() {
    let price: ModelPrice = serde_json::from_value(serde_json::json!({
        "input_per_mtok": 3_000_000,
        "cache_write_per_mtok": 3_750_000,
        "cache_write_5m_per_mtok": 4_000_000,
        "cache_write_1h_per_mtok": 6_000_000
    }))
    .expect("optional TTL rates are valid prices");
    let usage = Usage {
        input_tokens: 3_000_000,
        cache_write_tokens: 3_000_000,
        cache_write_5m_tokens: 1_000_000,
        cache_write_1h_tokens: 1_000_000,
        ..Usage::default()
    };
    assert_eq!(price.cost_micros(&usage), 13_750_000);
    let legacy: ModelPrice = serde_json::from_str(r#"{"cache_write_per_mtok":3750000}"#).unwrap();
    assert_eq!(legacy.cost_micros(&usage), 11_250_000);
}

#[test]
fn ttl_details_cannot_bill_more_cache_tokens_than_the_aggregate() {
    let price: ModelPrice = serde_json::from_value(serde_json::json!({
        "cache_write_per_mtok": 3_750_000,
        "cache_write_5m_per_mtok": 4_000_000,
        "cache_write_1h_per_mtok": 6_000_000
    }))
    .unwrap();
    let usage = Usage {
        input_tokens: 1_000_000,
        cache_write_tokens: 1_000_000,
        cache_write_5m_tokens: 1_000_000,
        cache_write_1h_tokens: 1_000_000,
        ..Usage::default()
    };
    assert_eq!(price.cost_micros(&usage), 4_000_000);
}

#[test]
fn optional_ttl_rates_use_the_same_safe_price_bounds() {
    let price: ModelPrice = serde_json::from_value(serde_json::json!({
        "cache_write_1h_per_mtok": 9_000_000_000_000_001_u64
    }))
    .unwrap();
    assert!(price.validate().is_err());
}

#[test]
fn ttl_breakdown_does_not_discard_fractional_cost_between_cache_classes() {
    let price: ModelPrice = serde_json::from_value(serde_json::json!({
        "cache_write_per_mtok": 500_000,
        "cache_write_5m_per_mtok": 500_000,
        "cache_write_1h_per_mtok": 500_000
    }))
    .unwrap();
    let usage = Usage {
        cache_write_tokens: 2,
        cache_write_5m_tokens: 1,
        cache_write_1h_tokens: 1,
        ..Usage::default()
    };
    assert_eq!(price.cost_micros(&usage), 1);
}

#[test]
fn single_rate_projection_preserves_legacy_and_equal_ttl_prices() {
    let mut price = ModelPrice {
        cache_write_per_mtok: 3_750_000,
        ..Default::default()
    };
    assert_eq!(price.uniform_cache_write_rate(), Some(3_750_000));
    price.cache_write_1h_per_mtok = Some(3_750_000);
    assert_eq!(price.uniform_cache_write_rate(), Some(3_750_000));
    price.cache_write_5m_per_mtok = Some(3_750_000);
    assert_eq!(price.uniform_cache_write_rate(), Some(3_750_000));
    price.cache_write_1h_per_mtok = Some(6_000_000);
    assert_eq!(price.uniform_cache_write_rate(), None);
}
