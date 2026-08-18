use crate::{
    config::{ApiDialect, AuthConfig, ClientConfig, EgressMode},
    secrets::{SecretStore, store_set},
    south_provider_call::{
        CancellationDispositionV1, CommunityCallPolicyV1, CommunityCredentialResolverV1,
        IneligibleV1, PrepareProviderCallErrorV1, PreparedCommunityProviderCallV1,
        ProviderPackageEligibilityV1, RequestBodyModeV1, ResponseMetadataEligibilityV1,
        RolloutEligibilityV1, StableProviderCallFailureV1, build_direct_reqwest_transport_v1,
        execute_prepared_provider_call_v1, map_failure_v1, prepare_provider_call_v1,
    },
};
use south_contracts::{
    BufferedHttpResponseV1, ContractErrorV1, CredentialSlotV1, MAX_JSON_REQUEST_BODY_BYTES,
    PreparationErrorV1, TransportErrorV1,
};
use south_core::{
    AsyncHttpTransport, CredentialResolutionFuture, CredentialResolver, PreparedHttpRequestV1,
    ProviderCallErrorV1, SecretValue, TransportFuture,
};
use south_provider_conformance::{
    FAKE_BEARER_SECRET_V1, PROVIDER_CALL_CONFORMANCE_DEADLINE_OFFSET_V1, ProviderCallControlV1,
    ProviderCallFailureCodeV1, ProviderCallFixtureV1, ProviderCallUpstreamV1,
};
use south_testkit::{
    AssembledExecutionFutureV1, AssembledProviderCallExecutorV1, ProviderCallEvidenceV1,
    ProviderCallObservationV1, run_provider_call_conformance_v1,
};
use std::{
    future::pending,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use token_station_protocol::{
    Auth, ErrorCode, HttpMethod, HttpRequestDescriptor, ProviderConfig, ProviderEndpoint,
    SafeHeaders, SecretRef,
};
use tokio::sync::{Notify, oneshot};
use tokio_util::sync::CancellationToken;

fn eligible_policy() -> CommunityCallPolicyV1 {
    CommunityCallPolicyV1::new(
        RolloutEligibilityV1::Enabled,
        ProviderPackageEligibilityV1::Approved,
        ApiDialect::Translated,
        EgressMode::Direct,
        RequestBodyModeV1::Buffered,
        ResponseMetadataEligibilityV1::Compatible,
    )
}

fn provider_config() -> ProviderConfig {
    let mut config = ProviderConfig::new(
        "openai-compatible",
        ProviderEndpoint::try_new("https://api.example.test/v1").expect("test endpoint is valid"),
    );
    config.auth = Some(SecretRef::new("provider_api_key"));
    config
}

fn auth_config() -> AuthConfig {
    AuthConfig {
        slot: "provider_api_key".to_owned(),
        store: true,
        env: None,
        file: None,
    }
}

fn descriptor() -> HttpRequestDescriptor {
    let mut descriptor = HttpRequestDescriptor::new(
        HttpMethod::Post,
        "https://api.example.test/v1/chat/completions",
    );
    descriptor.headers = SafeHeaders::try_new([
        ("content-type", "application/json"),
        ("x-trace-id", "trace-1"),
    ])
    .expect("test headers are valid");
    descriptor.body = Some(serde_json::json!({"model": "test"}));
    descriptor.auth = Some(Auth::bearer(SecretRef::new("provider_api_key")));
    descriptor
}

#[test]
fn eligible_descriptor_projects_into_the_south_contract() {
    let prepared = prepare_provider_call_v1(
        eligible_policy(),
        &provider_config(),
        &auth_config(),
        &descriptor(),
    )
    .expect("eligible descriptor should project");

    assert_eq!(prepared.relative_path(), "chat/completions");
    assert_eq!(prepared.credential_slot(), "provider_api_key");
    assert_eq!(prepared.header_count(), 2);
    assert_eq!(prepared.body(), r#"{"model":"test"}"#);
}

#[test]
fn unsupported_shapes_are_closed_host_local_reasons() {
    let cases = [
        (
            CommunityCallPolicyV1::new(
                RolloutEligibilityV1::Disabled,
                ProviderPackageEligibilityV1::Approved,
                ApiDialect::Translated,
                EgressMode::Direct,
                RequestBodyModeV1::Buffered,
                ResponseMetadataEligibilityV1::Compatible,
            ),
            IneligibleV1::RolloutDisabled,
        ),
        (
            CommunityCallPolicyV1::new(
                RolloutEligibilityV1::Enabled,
                ProviderPackageEligibilityV1::Unapproved,
                ApiDialect::Translated,
                EgressMode::Direct,
                RequestBodyModeV1::Buffered,
                ResponseMetadataEligibilityV1::Compatible,
            ),
            IneligibleV1::ProviderPackageUnapproved,
        ),
        (
            CommunityCallPolicyV1::new(
                RolloutEligibilityV1::Enabled,
                ProviderPackageEligibilityV1::Approved,
                ApiDialect::AnthropicNative,
                EgressMode::Direct,
                RequestBodyModeV1::Buffered,
                ResponseMetadataEligibilityV1::Compatible,
            ),
            IneligibleV1::ApiDialect,
        ),
        (
            CommunityCallPolicyV1::new(
                RolloutEligibilityV1::Enabled,
                ProviderPackageEligibilityV1::Approved,
                ApiDialect::Translated,
                EgressMode::Http,
                RequestBodyModeV1::Buffered,
                ResponseMetadataEligibilityV1::Compatible,
            ),
            IneligibleV1::Egress,
        ),
        (
            CommunityCallPolicyV1::new(
                RolloutEligibilityV1::Enabled,
                ProviderPackageEligibilityV1::Approved,
                ApiDialect::Translated,
                EgressMode::Direct,
                RequestBodyModeV1::Streaming,
                ResponseMetadataEligibilityV1::Compatible,
            ),
            IneligibleV1::Streaming,
        ),
    ];

    for (policy, expected) in cases {
        let error =
            prepare_provider_call_v1(policy, &provider_config(), &auth_config(), &descriptor())
                .expect_err("unsupported policy must not project");
        assert_eq!(error, PrepareProviderCallErrorV1::Ineligible(expected));
    }
}

#[test]
fn unsupported_descriptor_and_secret_shapes_are_closed_host_local_reasons() {
    let mut wrong_provider = provider_config();
    wrong_provider.provider = "anthropic".to_owned();
    let error = prepare_provider_call_v1(
        eligible_policy(),
        &wrong_provider,
        &auth_config(),
        &descriptor(),
    )
    .expect_err("a different provider dialect must not project");
    assert_eq!(
        error,
        PrepareProviderCallErrorV1::Ineligible(IneligibleV1::ProviderDialect)
    );

    let mut get = descriptor();
    get.method = HttpMethod::Get;
    let error =
        prepare_provider_call_v1(eligible_policy(), &provider_config(), &auth_config(), &get)
            .expect_err("GET must not project");
    assert_eq!(
        error,
        PrepareProviderCallErrorV1::Ineligible(IneligibleV1::Method)
    );

    let mut no_body = descriptor();
    no_body.body = None;
    let error = prepare_provider_call_v1(
        eligible_policy(),
        &provider_config(),
        &auth_config(),
        &no_body,
    )
    .expect_err("a missing body must not project");
    assert_eq!(
        error,
        PrepareProviderCallErrorV1::Ineligible(IneligibleV1::Body)
    );

    let mut header_auth = descriptor();
    header_auth.auth = Some(
        Auth::header("x-api-key", SecretRef::new("provider_api_key"))
            .expect("credential header is valid"),
    );
    let error = prepare_provider_call_v1(
        eligible_policy(),
        &provider_config(),
        &auth_config(),
        &header_auth,
    )
    .expect_err("Header auth must not project");
    assert_eq!(
        error,
        PrepareProviderCallErrorV1::Ineligible(IneligibleV1::Auth)
    );

    let file_auth = AuthConfig {
        slot: "provider_api_key".to_owned(),
        store: false,
        env: None,
        file: Some("secret.txt".into()),
    };
    let error = prepare_provider_call_v1(
        eligible_policy(),
        &provider_config(),
        &file_auth,
        &descriptor(),
    )
    .expect_err("synchronous file resolution must not project");
    assert_eq!(
        error,
        PrepareProviderCallErrorV1::Ineligible(IneligibleV1::SecretSource)
    );
    let empty_secrets = SecretStore::default();
    let resolver =
        CommunityCredentialResolverV1::try_new(&empty_secrets, "openai_personal", &file_auth);
    assert_eq!(
        resolver.expect_err("resolver construction must independently reject file sources"),
        IneligibleV1::SecretSource
    );

    let metadata_incompatible = CommunityCallPolicyV1::new(
        RolloutEligibilityV1::Enabled,
        ProviderPackageEligibilityV1::Approved,
        ApiDialect::Translated,
        EgressMode::Direct,
        RequestBodyModeV1::Buffered,
        ResponseMetadataEligibilityV1::Incompatible,
    );
    let error = prepare_provider_call_v1(
        metadata_incompatible,
        &provider_config(),
        &auth_config(),
        &descriptor(),
    )
    .expect_err("plugins needing more response metadata must not project");
    assert_eq!(
        error,
        PrepareProviderCallErrorV1::Ineligible(IneligibleV1::ResponseMetadata)
    );
}

#[test]
fn url_projection_rechecks_raw_path_origin_and_canonical_destination() {
    let cases = [
        (
            "https://other.example.test/v1/chat/completions",
            PrepareProviderCallErrorV1::Preparation(PreparationErrorV1::UrlOutsideBinding),
        ),
        (
            "https://api.example.test/v1beta/chat/completions",
            PrepareProviderCallErrorV1::Preparation(PreparationErrorV1::UrlOutsideBinding),
        ),
        (
            "https://api.example.test/v1/chat/completions?key=value",
            PrepareProviderCallErrorV1::Preparation(PreparationErrorV1::UrlOutsideBinding),
        ),
        (
            "https://api.example.test/v1/chat/completions#fragment",
            PrepareProviderCallErrorV1::Preparation(PreparationErrorV1::UrlOutsideBinding),
        ),
        (
            "https://user@api.example.test/v1/chat/completions",
            PrepareProviderCallErrorV1::Preparation(PreparationErrorV1::UrlOutsideBinding),
        ),
        (
            "https://api.example.test/v1/../escape",
            PrepareProviderCallErrorV1::Contract(ContractErrorV1::InvalidRelativePath),
        ),
        (
            "https://api.example.test/v1/a//b",
            PrepareProviderCallErrorV1::Contract(ContractErrorV1::InvalidRelativePath),
        ),
        (
            "https://api.example.test/v1/a%2fb",
            PrepareProviderCallErrorV1::Contract(ContractErrorV1::InvalidRelativePath),
        ),
        (
            "https://api.example.test/v1/a%252fb",
            PrepareProviderCallErrorV1::Contract(ContractErrorV1::InvalidRelativePath),
        ),
        (
            "https://api.example.test/v1/https:escape",
            PrepareProviderCallErrorV1::Contract(ContractErrorV1::InvalidRelativePath),
        ),
    ];

    for (url, expected) in cases {
        let mut candidate = descriptor();
        candidate.url = url.to_owned();
        let error = prepare_provider_call_v1(
            eligible_policy(),
            &provider_config(),
            &auth_config(),
            &candidate,
        )
        .expect_err("unsafe destination must fail before execution");
        assert_eq!(error, expected, "unexpected classification for {url}");
    }

    let mut default_port = descriptor();
    default_port.url = "https://api.example.test:443/v1/chat/completions".to_owned();
    let error = prepare_provider_call_v1(
        eligible_policy(),
        &provider_config(),
        &auth_config(),
        &default_port,
    )
    .expect_err("the existing host gate deliberately keeps authority spelling exact");
    assert_eq!(
        error,
        PrepareProviderCallErrorV1::Preparation(PreparationErrorV1::UrlOutsideBinding)
    );

    let mut explicit_port_provider = ProviderConfig::new(
        "openai-compatible",
        ProviderEndpoint::try_new("https://api.example.test:443/v1")
            .expect("explicit default port endpoint is valid"),
    );
    explicit_port_provider.auth = Some(SecretRef::new("provider_api_key"));
    prepare_provider_call_v1(
        eligible_policy(),
        &explicit_port_provider,
        &auth_config(),
        &default_port,
    )
    .expect("matching explicit authority passes both host and South gates");
}

#[test]
fn invalid_slots_headers_and_oversized_bodies_fail_before_capabilities_exist() {
    let mut invalid_slot = descriptor();
    invalid_slot.auth = Some(Auth::bearer(SecretRef::new("Invalid Slot")));
    let error = prepare_provider_call_v1(
        eligible_policy(),
        &provider_config(),
        &auth_config(),
        &invalid_slot,
    )
    .expect_err("invalid slot must fail during projection");
    assert_eq!(
        error,
        PrepareProviderCallErrorV1::Contract(ContractErrorV1::InvalidCredentialSlot)
    );

    let mut reserved_header = descriptor();
    reserved_header.headers = SafeHeaders::try_new([("user-agent", "provider-owned")])
        .expect("legacy policy permits this South-owned header");
    let error = prepare_provider_call_v1(
        eligible_policy(),
        &provider_config(),
        &auth_config(),
        &reserved_header,
    )
    .expect_err("South transport-owned header must fail closed");
    assert_eq!(
        error,
        PrepareProviderCallErrorV1::Ineligible(IneligibleV1::Headers)
    );

    let mut oversized = descriptor();
    oversized.body = Some(serde_json::Value::String(
        "x".repeat(MAX_JSON_REQUEST_BODY_BYTES),
    ));
    let error = prepare_provider_call_v1(
        eligible_policy(),
        &provider_config(),
        &auth_config(),
        &oversized,
    )
    .expect_err("serialized JSON beyond the South limit must fail closed");
    assert_eq!(
        error,
        PrepareProviderCallErrorV1::Contract(ContractErrorV1::RequestBodyTooLarge)
    );
}

#[test]
fn adapter_debug_surfaces_only_shape_and_counts() {
    let endpoint_sentinel = "endpoint-debug-sentinel.invalid";
    let path_sentinel = "path-debug-sentinel";
    let slot_sentinel = "slot-debug-sentinel";
    let header_sentinel = "header-debug-sentinel";
    let body_sentinel = "body-debug-sentinel";
    let mut provider = ProviderConfig::new(
        "openai-compatible",
        ProviderEndpoint::try_new(&format!("https://{endpoint_sentinel}/base"))
            .expect("sentinel endpoint is valid"),
    );
    provider.auth = Some(SecretRef::new(slot_sentinel));
    let auth = AuthConfig {
        slot: slot_sentinel.to_owned(),
        store: true,
        env: None,
        file: None,
    };
    let mut descriptor = HttpRequestDescriptor::new(
        HttpMethod::Post,
        format!("https://{endpoint_sentinel}/base/{path_sentinel}"),
    );
    descriptor.headers =
        SafeHeaders::try_new([("x-sentinel", header_sentinel)]).expect("sentinel header is valid");
    descriptor.body = Some(serde_json::json!({"value": body_sentinel}));
    descriptor.auth = Some(Auth::bearer(SecretRef::new(slot_sentinel)));

    let prepared = prepare_provider_call_v1(eligible_policy(), &provider, &auth, &descriptor)
        .expect("sentinel descriptor should project");
    let rendered = format!("{prepared:?}");
    for sentinel in [
        endpoint_sentinel,
        path_sentinel,
        slot_sentinel,
        header_sentinel,
        body_sentinel,
    ] {
        assert!(!rendered.contains(sentinel), "Debug leaked {sentinel}");
    }
}

struct ImmediateResolver {
    calls: AtomicUsize,
}

impl CredentialResolver for ImmediateResolver {
    fn resolve<'a>(&'a self, _slot: &'a CredentialSlotV1) -> CredentialResolutionFuture<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(SecretValue::new("synthetic-test-secret".to_owned())) })
    }
}

struct InspectingTransport {
    calls: AtomicUsize,
}

struct NeverTransport {
    calls: AtomicUsize,
}

impl AsyncHttpTransport for NeverTransport {
    fn execute<'a>(
        &'a self,
        _request: &'a PreparedHttpRequestV1<'_>,
        _remaining_timeout: Duration,
    ) -> TransportFuture<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Err(TransportErrorV1::RequestFailed) })
    }
}

impl AsyncHttpTransport for InspectingTransport {
    fn execute<'a>(
        &'a self,
        request: &'a PreparedHttpRequestV1<'_>,
        _remaining_timeout: Duration,
    ) -> TransportFuture<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            request.url().as_str(),
            "https://api.example.test/v1/chat/completions"
        );
        assert_eq!(request.body().as_str(), r#"{"model":"test"}"#);
        assert_eq!(request.headers().get("x-trace-id"), Some("trace-1"));
        assert_eq!(request.bearer_secret(), b"synthetic-test-secret");
        Box::pin(async {
            BufferedHttpResponseV1::try_from_parts(
                201_u16.try_into().expect("test status is valid"),
                br#"{"ok":true}"#.to_vec(),
                Some("application/json".to_owned()),
                Some("2".to_owned()),
            )
        })
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn assembled_execution_uses_real_south_core_and_projects_the_response() {
    let prepared = prepare_provider_call_v1(
        eligible_policy(),
        &provider_config(),
        &auth_config(),
        &descriptor(),
    )
    .expect("eligible descriptor should project");
    let resolver = ImmediateResolver {
        calls: AtomicUsize::new(0),
    };
    let transport = InspectingTransport {
        calls: AtomicUsize::new(0),
    };
    let cancellation = CancellationToken::new();

    let response = execute_prepared_provider_call_v1(
        &prepared,
        &resolver,
        &transport,
        tokio::time::Instant::now() + Duration::from_secs(30),
        &cancellation,
    )
    .await
    .expect("assembled execution should succeed");

    assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
    assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    assert_eq!(response.status, 201);
    assert_eq!(response.body, r#"{"ok":true}"#);
    assert_eq!(
        response.headers.get("content-type").map(String::as_str),
        Some("application/json")
    );
    assert_eq!(
        response.headers.get("retry-after").map(String::as_str),
        Some("2")
    );
    assert!(response.extensions.is_empty());
}

#[test]
fn every_south_v1_contract_failure_has_a_closed_community_mapping() {
    let contract = [
        (ContractErrorV1::InvalidEndpoint, ErrorCode::Internal, 500),
        (
            ContractErrorV1::InvalidRelativePath,
            ErrorCode::Internal,
            500,
        ),
        (
            ContractErrorV1::InvalidCredentialSlot,
            ErrorCode::Internal,
            500,
        ),
        (ContractErrorV1::InvalidJsonBody, ErrorCode::Internal, 500),
        (
            ContractErrorV1::RequestBodyTooLarge,
            ErrorCode::InvalidRequest,
            413,
        ),
    ];
    for (failure, code, status) in contract {
        let mapped = map_failure_v1(
            StableProviderCallFailureV1::Contract(failure),
            CancellationDispositionV1::ClientDisconnected,
        );
        assert_eq!((mapped.code, mapped.http_status), (code, status));
    }
}

#[test]
fn every_south_v1_preparation_failure_has_a_closed_community_mapping() {
    let preparation = [
        (
            PreparationErrorV1::UrlOutsideBinding,
            ErrorCode::Internal,
            500,
        ),
        (
            PreparationErrorV1::CredentialBindingMismatch,
            ErrorCode::Internal,
            500,
        ),
        (
            PreparationErrorV1::CredentialResolutionFailed,
            ErrorCode::Auth,
            401,
        ),
        (
            PreparationErrorV1::DeadlineExceeded,
            ErrorCode::Timeout,
            504,
        ),
    ];
    for (failure, code, status) in preparation {
        let mapped = map_failure_v1(
            StableProviderCallFailureV1::Provider(ProviderCallErrorV1::Preparation(failure)),
            CancellationDispositionV1::ClientDisconnected,
        );
        assert_eq!((mapped.code, mapped.http_status), (code, status));
    }

    for (reason, code, status) in [
        (
            CancellationDispositionV1::ClientDisconnected,
            ErrorCode::Internal,
            499,
        ),
        (
            CancellationDispositionV1::ServerDrain,
            ErrorCode::UpstreamUnavailable,
            503,
        ),
        (CancellationDispositionV1::Deadline, ErrorCode::Timeout, 504),
    ] {
        let mapped = map_failure_v1(
            StableProviderCallFailureV1::Provider(ProviderCallErrorV1::Preparation(
                PreparationErrorV1::Cancelled,
            )),
            reason,
        );
        assert_eq!((mapped.code, mapped.http_status), (code, status));
    }
}

#[test]
fn every_south_v1_transport_failure_has_a_closed_community_mapping() {
    let transport = [
        (
            TransportErrorV1::ClientBuildFailed,
            ErrorCode::Internal,
            500,
        ),
        (TransportErrorV1::TransportTimeout, ErrorCode::Timeout, 504),
        (
            TransportErrorV1::ConnectFailed,
            ErrorCode::UpstreamUnavailable,
            502,
        ),
        (
            TransportErrorV1::RequestFailed,
            ErrorCode::UpstreamUnavailable,
            502,
        ),
        (
            TransportErrorV1::ResponseReadFailed,
            ErrorCode::TransportTruncated,
            502,
        ),
        (
            TransportErrorV1::ResponseBodyTooLarge,
            ErrorCode::ProviderProtocolError,
            502,
        ),
        (
            TransportErrorV1::ResponseBodyNotUtf8,
            ErrorCode::ProviderProtocolError,
            502,
        ),
        (
            TransportErrorV1::ResponseMetadataInvalid,
            ErrorCode::ProviderProtocolError,
            502,
        ),
        (
            TransportErrorV1::RedirectDenied,
            ErrorCode::UpstreamUnavailable,
            502,
        ),
    ];
    for (failure, code, status) in transport {
        let mapped = map_failure_v1(
            StableProviderCallFailureV1::Provider(ProviderCallErrorV1::Transport(failure)),
            CancellationDispositionV1::ClientDisconnected,
        );
        assert_eq!((mapped.code, mapped.http_status), (code, status));
    }
}

fn store_resolver(value: &str) -> (SecretStore, std::path::PathBuf) {
    let directory = std::env::temp_dir().join(format!(
        "token-station-south-adapter-{}-{}",
        std::process::id(),
        value.len()
    ));
    std::fs::create_dir_all(&directory).expect("test secret directory should exist");
    store_set(&directory, "openai_personal", "provider_api_key", value)
        .expect("test secret should be stored");
    let mut config = ClientConfig::parse_with_load_migrations(crate::EXAMPLE_CONFIG)
        .expect("example config should parse");
    config
        .upstreams
        .get_mut("openai_personal")
        .expect("example upstream should exist")
        .auth = Some(auth_config());
    (SecretStore::from_config(&config, &directory), directory)
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn community_resolver_enforces_slot_and_secret_size_without_background_work() {
    let (secrets, directory) = store_resolver("synthetic-test-secret");
    let resolver_auth = auth_config();
    let resolver =
        CommunityCredentialResolverV1::try_new(&secrets, "openai_personal", &resolver_auth)
            .expect("store-backed resolver should be eligible");
    let slot = CredentialSlotV1::parse("provider_api_key").expect("test slot is valid");
    let other_slot = CredentialSlotV1::parse("other_key").expect("test slot is valid");

    assert!(resolver.resolve(&slot).await.is_ok());
    assert!(resolver.resolve(&other_slot).await.is_err());
    assert!(!format!("{resolver:?}").contains("openai_personal"));
    assert!(!format!("{resolver:?}").contains("provider_api_key"));
    std::fs::remove_dir_all(directory).expect("test secret directory should be removable");

    let oversized = "x".repeat(16 * 1024 + 1);
    let (secrets, directory) = store_resolver(&oversized);
    let resolver_auth = auth_config();
    let resolver =
        CommunityCredentialResolverV1::try_new(&secrets, "openai_personal", &resolver_auth)
            .expect("store-backed resolver should be eligible");
    let prepared = prepare_provider_call_v1(
        eligible_policy(),
        &provider_config(),
        &auth_config(),
        &descriptor(),
    )
    .expect("eligible descriptor should project");
    let transport = NeverTransport {
        calls: AtomicUsize::new(0),
    };
    let failure = execute_prepared_provider_call_v1(
        &prepared,
        &resolver,
        &transport,
        tokio::time::Instant::now() + Duration::from_secs(30),
        &CancellationToken::new(),
    )
    .await
    .expect_err("oversized secret must fail resolution");
    assert_eq!(
        failure,
        StableProviderCallFailureV1::Provider(ProviderCallErrorV1::Preparation(
            PreparationErrorV1::CredentialResolutionFailed,
        ))
    );
    assert_eq!(transport.calls.load(Ordering::SeqCst), 0);
    std::fs::remove_dir_all(directory).expect("test secret directory should be removable");
}

#[test]
fn direct_reqwest_transport_is_built_only_from_explicit_bounded_timeouts() {
    let transport = build_direct_reqwest_transport_v1(
        Duration::from_secs(30),
        Duration::from_secs(5),
        Duration::from_secs(5),
    )
    .expect("valid explicit timeouts should build a dedicated transport");
    assert_eq!(format!("{transport:?}"), "ReqwestTransportV1 { .. }");

    let failure = build_direct_reqwest_transport_v1(
        Duration::ZERO,
        Duration::from_secs(1),
        Duration::from_secs(1),
    )
    .expect_err("zero total timeout must fail closed");
    assert_eq!(
        failure,
        StableProviderCallFailureV1::Provider(ProviderCallErrorV1::Transport(
            TransportErrorV1::ClientBuildFailed,
        ))
    );
}

struct CommunityConformanceExecutorV1 {
    deadline_transport_started: Arc<Notify>,
}

fn prepare_conformance_fixture(
    fixture: &ProviderCallFixtureV1,
) -> Result<PreparedCommunityProviderCallV1, PrepareProviderCallErrorV1> {
    let input = fixture.input();
    let mut provider = ProviderConfig::new(
        "openai-compatible",
        ProviderEndpoint::try_new(input.endpoint()).expect("canonical endpoint is valid"),
    );
    provider.auth = Some(SecretRef::new(input.bound_credential_slot()));
    let auth = AuthConfig {
        slot: input.bound_credential_slot().to_owned(),
        store: true,
        env: None,
        file: None,
    };
    let mut descriptor = HttpRequestDescriptor::new(
        HttpMethod::Post,
        format!(
            "{}/{}",
            input.endpoint().trim_end_matches('/'),
            input.relative_path()
        ),
    );
    descriptor.headers =
        SafeHeaders::try_new(input.headers().iter().copied()).expect("canonical headers are valid");
    descriptor.body =
        Some(serde_json::from_str(input.json_body()).expect("canonical request body is valid"));
    descriptor.auth = Some(Auth::bearer(SecretRef::new(
        input.requested_credential_slot(),
    )));

    prepare_provider_call_v1(eligible_policy(), &provider, &auth, &descriptor)
}

impl CommunityConformanceExecutorV1 {
    fn new() -> Self {
        Self {
            deadline_transport_started: Arc::new(Notify::new()),
        }
    }

    async fn deadline_transport_started(&self) {
        self.deadline_transport_started.notified().await;
    }

    async fn execute_community_case(
        &self,
        fixture: &ProviderCallFixtureV1,
    ) -> ProviderCallObservationV1 {
        let resolver_calls = Arc::new(AtomicUsize::new(0));
        let transport_calls = Arc::new(AtomicUsize::new(0));
        let resolver_dropped = Arc::new(AtomicBool::new(false));
        let transport_dropped = Arc::new(AtomicBool::new(false));

        let prepared = match prepare_conformance_fixture(fixture) {
            Ok(prepared) => prepared,
            Err(error) => {
                return ProviderCallObservationV1::failure(
                    map_prepare_failure_code(error),
                    ProviderCallEvidenceV1::new(0, 0, false, false),
                );
            }
        };

        let cancellation = CancellationToken::new();
        let (resolver_started_sender, resolver_started_receiver) = oneshot::channel();
        let resolver = ConformanceResolver {
            calls: Arc::clone(&resolver_calls),
            pending: matches!(
                fixture.control(),
                ProviderCallControlV1::CancelWhileResolverPending
            ),
            started: Mutex::new(Some(resolver_started_sender)),
            dropped: Arc::clone(&resolver_dropped),
        };
        let transport = ConformanceTransport {
            calls: Arc::clone(&transport_calls),
            upstream: fixture.upstream(),
            deadline_started: Arc::clone(&self.deadline_transport_started),
            dropped: Arc::clone(&transport_dropped),
        };
        let deadline = tokio::time::Instant::now()
            + if matches!(
                fixture.control(),
                ProviderCallControlV1::ExpireWhileTransportPending
            ) {
                PROVIDER_CALL_CONFORMANCE_DEADLINE_OFFSET_V1
            } else {
                Duration::from_secs(30)
            };

        let result = if matches!(
            fixture.control(),
            ProviderCallControlV1::CancelWhileResolverPending
        ) {
            let call = execute_prepared_provider_call_v1(
                &prepared,
                &resolver,
                &transport,
                deadline,
                &cancellation,
            );
            let cancel = async {
                let _ = resolver_started_receiver.await;
                cancellation.cancel();
            };
            let (result, ()) = tokio::join!(call, cancel);
            result
        } else {
            execute_prepared_provider_call_v1(
                &prepared,
                &resolver,
                &transport,
                deadline,
                &cancellation,
            )
            .await
        };

        let evidence = ProviderCallEvidenceV1::new(
            resolver_calls.load(Ordering::SeqCst),
            transport_calls.load(Ordering::SeqCst),
            resolver_dropped.load(Ordering::SeqCst),
            transport_dropped.load(Ordering::SeqCst),
        );
        match result {
            Ok(response) => {
                let content_type = response.headers.get("content-type").cloned();
                let retry_after = response.headers.get("retry-after").cloned();
                let bounded = BufferedHttpResponseV1::try_from_parts(
                    response
                        .status
                        .try_into()
                        .expect("canonical response status is valid"),
                    response.body.into_bytes(),
                    content_type,
                    retry_after,
                )
                .expect("adapter response remains inside the South response contract");
                ProviderCallObservationV1::response(bounded, evidence)
            }
            Err(error) => {
                ProviderCallObservationV1::failure(map_stable_failure_code(error), evidence)
            }
        }
    }
}

impl AssembledProviderCallExecutorV1 for CommunityConformanceExecutorV1 {
    fn execute_case<'a>(
        &'a self,
        fixture: &'a ProviderCallFixtureV1,
    ) -> AssembledExecutionFutureV1<'a> {
        Box::pin(async move { self.execute_community_case(fixture).await })
    }
}

struct ConformanceResolver {
    calls: Arc<AtomicUsize>,
    pending: bool,
    started: Mutex<Option<oneshot::Sender<()>>>,
    dropped: Arc<AtomicBool>,
}

impl CredentialResolver for ConformanceResolver {
    fn resolve<'a>(&'a self, _slot: &'a CredentialSlotV1) -> CredentialResolutionFuture<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.pending {
            let started = self
                .started
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .take();
            let dropped = Arc::clone(&self.dropped);
            Box::pin(async move {
                let _drop_probe = PendingDropProbe(dropped);
                if let Some(started) = started {
                    let _ = started.send(());
                }
                pending().await
            })
        } else {
            Box::pin(async { Ok(SecretValue::new(FAKE_BEARER_SECRET_V1.to_owned())) })
        }
    }
}

struct ConformanceTransport<'fixture> {
    calls: Arc<AtomicUsize>,
    upstream: &'fixture ProviderCallUpstreamV1,
    deadline_started: Arc<Notify>,
    dropped: Arc<AtomicBool>,
}

impl AsyncHttpTransport for ConformanceTransport<'_> {
    fn execute<'a>(
        &'a self,
        _request: &'a PreparedHttpRequestV1<'_>,
        _remaining_timeout: Duration,
    ) -> TransportFuture<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.upstream {
            ProviderCallUpstreamV1::Response(raw) => {
                let raw = *raw;
                Box::pin(async move {
                    BufferedHttpResponseV1::try_from_parts(
                        raw.status()
                            .try_into()
                            .map_err(|_| TransportErrorV1::ResponseMetadataInvalid)?,
                        raw.body().as_bytes().to_vec(),
                        raw.content_type().map(str::to_owned),
                        raw.retry_after().map(str::to_owned),
                    )
                })
            }
            ProviderCallUpstreamV1::TransportFailure(error) => {
                let error = *error;
                Box::pin(async move { Err(error) })
            }
            ProviderCallUpstreamV1::Pending => {
                let started = Arc::clone(&self.deadline_started);
                let dropped = Arc::clone(&self.dropped);
                Box::pin(async move {
                    let _drop_probe = PendingDropProbe(dropped);
                    started.notify_one();
                    pending().await
                })
            }
            ProviderCallUpstreamV1::NotReached => {
                Box::pin(async { Err(TransportErrorV1::RequestFailed) })
            }
        }
    }
}

struct PendingDropProbe(Arc<AtomicBool>);

impl Drop for PendingDropProbe {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

fn map_prepare_failure_code(error: PrepareProviderCallErrorV1) -> ProviderCallFailureCodeV1 {
    match error {
        PrepareProviderCallErrorV1::Ineligible(_) => ProviderCallFailureCodeV1::RequestFailed,
        PrepareProviderCallErrorV1::Contract(error) => map_contract_failure_code(error),
        PrepareProviderCallErrorV1::Preparation(error) => map_preparation_failure_code(error),
    }
}

fn map_stable_failure_code(error: StableProviderCallFailureV1) -> ProviderCallFailureCodeV1 {
    match error {
        StableProviderCallFailureV1::Contract(error) => map_contract_failure_code(error),
        StableProviderCallFailureV1::Provider(ProviderCallErrorV1::Preparation(error)) => {
            map_preparation_failure_code(error)
        }
        StableProviderCallFailureV1::Provider(ProviderCallErrorV1::Transport(error)) => match error
        {
            TransportErrorV1::ClientBuildFailed => ProviderCallFailureCodeV1::ClientBuildFailed,
            TransportErrorV1::TransportTimeout => ProviderCallFailureCodeV1::TransportTimeout,
            TransportErrorV1::ConnectFailed => ProviderCallFailureCodeV1::ConnectFailed,
            TransportErrorV1::RequestFailed => ProviderCallFailureCodeV1::RequestFailed,
            TransportErrorV1::ResponseReadFailed => ProviderCallFailureCodeV1::ResponseReadFailed,
            TransportErrorV1::ResponseBodyTooLarge => {
                ProviderCallFailureCodeV1::ResponseBodyTooLarge
            }
            TransportErrorV1::ResponseBodyNotUtf8 => ProviderCallFailureCodeV1::ResponseBodyNotUtf8,
            TransportErrorV1::ResponseMetadataInvalid => {
                ProviderCallFailureCodeV1::ResponseMetadataInvalid
            }
            TransportErrorV1::RedirectDenied => ProviderCallFailureCodeV1::RedirectDenied,
        },
    }
}

fn map_contract_failure_code(error: ContractErrorV1) -> ProviderCallFailureCodeV1 {
    match error {
        ContractErrorV1::InvalidEndpoint => ProviderCallFailureCodeV1::InvalidEndpoint,
        ContractErrorV1::InvalidRelativePath => ProviderCallFailureCodeV1::InvalidRelativePath,
        ContractErrorV1::InvalidCredentialSlot => ProviderCallFailureCodeV1::InvalidCredentialSlot,
        ContractErrorV1::InvalidJsonBody => ProviderCallFailureCodeV1::InvalidJsonBody,
        ContractErrorV1::RequestBodyTooLarge => ProviderCallFailureCodeV1::RequestBodyTooLarge,
    }
}

fn map_preparation_failure_code(error: PreparationErrorV1) -> ProviderCallFailureCodeV1 {
    match error {
        PreparationErrorV1::UrlOutsideBinding => ProviderCallFailureCodeV1::UrlOutsideBinding,
        PreparationErrorV1::CredentialBindingMismatch => {
            ProviderCallFailureCodeV1::CredentialBindingMismatch
        }
        PreparationErrorV1::CredentialResolutionFailed => {
            ProviderCallFailureCodeV1::CredentialResolutionFailed
        }
        PreparationErrorV1::Cancelled => ProviderCallFailureCodeV1::Cancelled,
        PreparationErrorV1::DeadlineExceeded => ProviderCallFailureCodeV1::DeadlineExceeded,
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn community_adapter_passes_all_seven_public_south_cases() {
    let executor = CommunityConformanceExecutorV1::new();
    let structured_run = async {
        let runner = run_provider_call_conformance_v1(&executor);
        let deadline_driver = async {
            executor.deadline_transport_started().await;
            tokio::time::advance(PROVIDER_CALL_CONFORMANCE_DEADLINE_OFFSET_V1).await;
        };
        tokio::join!(runner, deadline_driver)
    };

    let (report, ()) = tokio::time::timeout(Duration::from_secs(5), structured_run)
        .await
        .expect("structured conformance watchdog expired");
    let report = report.expect("community adapter must pass the public suite");
    assert_eq!(report.passed_case_ids().len(), 7);
}
