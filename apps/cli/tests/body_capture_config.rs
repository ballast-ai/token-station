use token_station_cli::config::DataConfig;

#[test]
fn legacy_capture_is_preserved_and_metadata_only_is_explicit() {
    let legacy: DataConfig = serde_json::from_str("{}").unwrap();
    let serialized = serde_json::to_value(legacy).unwrap();
    assert_eq!(serialized["request_body_capture"], true);
    let private: DataConfig = serde_json::from_str(r#"{"request_body_capture":false}"#)
        .expect("metadata-only capture policy is a supported setting");
    assert_eq!(
        serde_json::to_value(private).unwrap()["request_body_capture"],
        false
    );
}
