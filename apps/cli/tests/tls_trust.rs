//! End-to-end trust behavior for `egress.extra_ca_file`: a real rustls
//! server with a private-CA-signed leaf, a real ureq handshake.
//!
//! Public behavior pinned here:
//! - Without the extra CA the request fails and classifies as
//!   `UntrustedIssuer` (the actionable "configure a CA" case).
//! - With the extra CA configured the same request succeeds — while the
//!   webpki roots stay merged in (asserted structurally in the loader).

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;

use token_station_cli::config::EgressConfig;
use token_station_cli::tls_trust::{classify_tls_failure, TlsTrustFailure};

const CA_PEM: &[u8] = include_bytes!("fixtures/tls/ca_cert.pem");
const SERVER_CERT_PEM: &[u8] = include_bytes!("fixtures/tls/server_cert.pem");
const SERVER_KEY_PEM: &[u8] = include_bytes!("fixtures/tls/server_key.pem");

fn pem_certificates(pem: &[u8]) -> Vec<rustls::pki_types::CertificateDer<'static>> {
    ureq::tls::parse_pem(pem)
        .filter_map(|item| match item.expect("fixture PEM parses") {
            ureq::tls::PemItem::Certificate(cert) => {
                Some(rustls::pki_types::CertificateDer::from(cert.der().to_vec()))
            }
            _ => None,
        })
        .collect()
}

/// One-shot HTTPS server: accepts connections until the listener is dropped
/// with the process, answers every completed handshake with a fixed 200.
fn spawn_tls_server() -> SocketAddr {
    let key = ureq::tls::PrivateKey::from_pem(SERVER_KEY_PEM).expect("fixture key parses");
    let key = rustls::pki_types::PrivateKeyDer::try_from(key.der().to_vec())
        .expect("fixture key is DER");
    let config = rustls::ServerConfig::builder_with_provider(Arc::new(
        rustls::crypto::ring::default_provider(),
    ))
    .with_safe_default_protocol_versions()
    .expect("protocol versions")
    .with_no_client_auth()
    .with_single_cert(pem_certificates(SERVER_CERT_PEM), key)
    .expect("fixture server certificate");
    let config = Arc::new(config);
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind test listener");
    let addr = listener.local_addr().expect("listener address");
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut conn = rustls::ServerConnection::new(config.clone()).expect("server conn");
            let mut tls = rustls::Stream::new(&mut conn, &mut stream);
            // A failed handshake (untrusted client verdict) errors here; that
            // is the expected path of the no-CA test, so errors just move on
            // to the next connection.
            let mut request = [0u8; 1024];
            if tls.read(&mut request).is_err() {
                continue;
            }
            let _ = tls.write_all(
                b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nok",
            );
        }
    });
    addr
}

fn agent_for(egress: &EgressConfig) -> ureq::Agent {
    let mut builder = ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(10)))
        .http_status_as_error(false);
    if let Some(tls) = egress.extra_tls_config().expect("tls config builds") {
        builder = builder.tls_config(tls);
    }
    ureq::Agent::new_with_config(builder.build())
}

#[test]
fn private_ca_server_rejected_without_extra_ca_and_classified_as_untrusted() {
    let addr = spawn_tls_server();
    let agent = agent_for(&EgressConfig::default());
    let error = agent
        .get(format!("https://localhost:{}/", addr.port()))
        .call()
        .expect_err("webpki roots must not trust the private test CA");
    assert_eq!(
        classify_tls_failure(&error),
        Some(TlsTrustFailure::UntrustedIssuer),
        "unexpected error shape: {error:?}"
    );
}

#[test]
fn private_ca_server_accepted_with_extra_ca_file() {
    let addr = spawn_tls_server();
    let ca_path = std::env::temp_dir().join(format!(
        "token-station-extra-ca-{}.pem",
        std::process::id()
    ));
    std::fs::write(&ca_path, CA_PEM).expect("write CA fixture");
    let egress = EgressConfig {
        extra_ca_file: Some(ca_path.display().to_string()),
        ..EgressConfig::default()
    };
    let response = agent_for(&egress)
        .get(format!("https://localhost:{}/", addr.port()))
        .call()
        .expect("handshake succeeds once the private CA is trusted");
    assert_eq!(response.status(), 200);
    let _ = std::fs::remove_file(&ca_path);
}
