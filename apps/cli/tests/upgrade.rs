//! The upgrade trust chain, exercised for real: a scripted "GitHub" serves a
//! release whose manifest was signed with a test key, and the client accepts
//! it only when every link holds — and refuses it when any link is cut.

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::json;
use token_station_cli::upgrade::{self, Release};
use token_station_release::{ReleaseManifest, hex, keygen, sign_bytes};

/// Serves fixed bodies by path, one connection at a time. GETs only — that
/// is all the upgrade path is allowed to make.
struct MockReleases {
    base: String,
}

impl MockReleases {
    fn start(routes: BTreeMap<String, Vec<u8>>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("loopback binds");
        let base = format!("http://{}", listener.local_addr().expect("bound"));
        let routes = Arc::new(routes);

        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let mut buffer = [0u8; 4096];
                let mut request = Vec::new();
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let Ok(read) = stream.read(&mut buffer) else {
                        break;
                    };
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..read]);
                }
                let head = String::from_utf8_lossy(&request);
                let path = head
                    .lines()
                    .next()
                    .and_then(|line| line.split(' ').nth(1))
                    .unwrap_or_default()
                    .to_owned();
                let response = match routes.get(&path) {
                    Some(body) => {
                        let mut response = format!(
                            "HTTP/1.1 200 OK\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                            body.len()
                        )
                        .into_bytes();
                        response.extend_from_slice(body);
                        response
                    }
                    None => b"HTTP/1.1 404 NF\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
                        .to_vec(),
                };
                let _ = stream.write_all(&response);
            }
        });

        Self { base }
    }
}

/// A whole scripted release: artifact, signed manifest, listing — plus the
/// key it was signed with and a scratch dir to download into.
struct Scene {
    release: Release,
    pubkey_hex: String,
    download_dir: PathBuf,
    artifact_name: String,
    _server: MockReleases,
}

/// `tamper`: mutates the served bodies after signing, to cut one link.
fn scene(name: &str, tamper: impl FnOnce(&mut BTreeMap<String, Vec<u8>>)) -> Scene {
    let dir = std::env::temp_dir().join(format!("ts-upgrade-{}-{name}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("temp dir");

    let triple = upgrade::target_triple().expect("tests run on a published platform");
    let artifact_name = format!("token-station-cli-9.9.9-{triple}.tar.gz");
    let artifact_path = dir.join(&artifact_name);
    std::fs::write(&artifact_path, b"pretend this is a tarball").expect("writes");

    let manifest =
        ReleaseManifest::for_files("9.9.9", 1_752_000_000, &[&artifact_path]).expect("hashes");
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("serializes");
    let key = keygen().expect("entropy");
    let signature = sign_bytes(&key, &manifest_bytes);

    let mut routes = BTreeMap::new();
    routes.insert(
        format!("/dl/{artifact_name}"),
        b"pretend this is a tarball".to_vec(),
    );
    routes.insert("/dl/manifest.json".to_owned(), manifest_bytes);
    routes.insert("/dl/manifest.json.sig".to_owned(), signature.into_bytes());
    tamper(&mut routes);

    let server = MockReleases::start(routes.clone());
    let asset = |name: &str| json!({ "name": name, "browser_download_url": format!("{}/dl/{name}", server.base) });
    let listing = json!({
        "tag_name": "v9.9.9",
        "html_url": "https://example.invalid/releases/v9.9.9",
        "assets": [asset(&artifact_name), asset("manifest.json"), asset("manifest.json.sig")]
    });

    // The listing itself goes through `check`, over the same mock wire.
    routes.insert(
        "/releases/latest".to_owned(),
        listing.to_string().into_bytes(),
    );
    let server = MockReleases::start(routes);
    let release = upgrade::check(&server.base).expect("the listing parses");

    let download_dir = dir.join("downloads");
    std::fs::create_dir_all(&download_dir).expect("temp dir");
    Scene {
        release,
        pubkey_hex: hex(key.verifying_key().as_bytes()),
        download_dir,
        artifact_name,
        _server: server,
    }
}

#[test]
fn a_signed_release_downloads_and_verifies() {
    let scene = scene("ok", |_| {});

    assert_eq!(scene.release.tag_name, "v9.9.9");
    assert!(upgrade::is_newer(
        upgrade::CURRENT_VERSION,
        &scene.release.tag_name
    ));

    let path = upgrade::download_and_verify(&scene.release, &scene.download_dir, &scene.pubkey_hex)
        .expect("every link holds");
    assert_eq!(
        std::fs::read(&path).expect("downloaded"),
        b"pretend this is a tarball"
    );
}

#[test]
fn a_swapped_artifact_is_discarded_with_the_hashes_named() {
    let scene = scene("swap", |routes| {
        for (path, body) in routes.iter_mut() {
            if path.ends_with(".tar.gz") {
                *body = b"same name, different bytes".to_vec();
            }
        }
    });

    let error =
        upgrade::download_and_verify(&scene.release, &scene.download_dir, &scene.pubkey_hex)
            .expect_err("the hash must catch the swap");
    assert!(error.contains("does not match"), "{error}");
    assert!(
        !scene.download_dir.join(&scene.artifact_name).exists(),
        "a failed download must not survive on disk"
    );
}

#[test]
fn a_failed_download_never_overwrites_or_deletes_an_existing_artifact() {
    let scene = scene("preserve-existing", |routes| {
        for (path, body) in routes.iter_mut() {
            if path.ends_with(".tar.gz") {
                *body = b"tampered replacement".to_vec();
            }
        }
    });
    let existing = scene.download_dir.join(&scene.artifact_name);
    std::fs::write(&existing, b"previously verified").expect("existing artifact");

    upgrade::download_and_verify(&scene.release, &scene.download_dir, &scene.pubkey_hex)
        .expect_err("an existing target may not be replaced by a failed download");
    assert_eq!(
        std::fs::read(existing).expect("existing artifact survives"),
        b"previously verified"
    );
}

#[test]
fn a_manifest_signed_by_someone_else_is_refused_before_any_artifact_downloads() {
    // The scene's manifest is signed by the scene's key; verifying against a
    // *different* key is the compromised-channel case.
    let scene = scene("wrong-key", |_| {});
    let other = keygen().expect("entropy");

    let error = upgrade::download_and_verify(
        &scene.release,
        &scene.download_dir,
        &hex(other.verifying_key().as_bytes()),
    )
    .expect_err("a foreign signature is worthless");
    assert!(error.contains("manifest does not verify"), "{error}");
    assert!(!scene.download_dir.join(&scene.artifact_name).exists());
}

#[test]
fn a_source_build_without_a_key_refuses_and_points_at_the_docs() {
    let scene = scene("no-key", |_| {});

    let error = upgrade::download_and_verify(&scene.release, &scene.download_dir, "")
        .expect_err("no key, no trust chain");
    assert!(error.contains("built from source"), "{error}");
    assert!(error.contains(&scene.release.html_url), "{error}");
}
