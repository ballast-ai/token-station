use std::fs;
use std::path::PathBuf;

fn main() {
    generate_connector_registry();
    tauri_build::build()
}

fn generate_connector_registry() {
    let manifest = PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("manifest dir"));
    let connector_dir = manifest.join("src/agent_integration/connectors");
    let mut modules = fs::read_dir(&connector_dir)
        .expect("read connector modules")
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let path = entry.path();
            let stem = path.file_stem()?.to_str()?.to_owned();
            (path.extension().and_then(|value| value.to_str()) == Some("rs") && stem != "mod")
                .then_some((stem, path))
        })
        .collect::<Vec<_>>();
    modules.sort_by(|left, right| left.0.cmp(&right.0));

    let mut generated = String::new();
    for (module, path) in &modules {
        generated.push_str(&format!("#[path = {:?}]\nmod {module};\n", path));
    }
    generated.push_str("\nstatic BUILTIN_CONNECTORS: &[&dyn Connector] = &[\n");
    for (module, _) in &modules {
        generated.push_str(&format!("    &{module}::CONNECTOR,\n"));
    }
    generated.push_str("];\n");

    let out = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
    fs::write(out.join("builtin_connectors.rs"), generated).expect("write connector registry");
    println!("cargo:rerun-if-changed={}", connector_dir.display());
}
