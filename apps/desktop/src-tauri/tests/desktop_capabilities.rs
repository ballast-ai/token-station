use std::path::Path;

#[test]
fn main_window_can_apply_the_selected_theme_without_broad_capability_expansion() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let bytes = std::fs::read(manifest_dir.join("capabilities/default.json"))
        .expect("desktop capability manifest must be readable");
    let manifest: serde_json::Value =
        serde_json::from_slice(&bytes).expect("desktop capability manifest must be valid JSON");
    let permissions = manifest["permissions"]
        .as_array()
        .expect("desktop permissions must be an array");

    assert!(
        permissions.iter().any(|permission| permission == "core:window:allow-set-theme"),
        "ThemeProvider calls window.setTheme on every launch, so the main window must explicitly allow set-theme",
    );
    assert!(
        !permissions
            .iter()
            .any(|permission| permission == "core:window:allow-set-*"),
        "theme synchronization must not grant every window mutation permission",
    );
}
