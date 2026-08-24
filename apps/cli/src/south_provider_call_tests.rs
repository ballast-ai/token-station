use std::collections::BTreeSet;

use south_provider_api::AuthArmV1;

use crate::{
    config::{ApiDialect, AuthConfig, ClientConfig, EgressMode},
    secrets::{SecretStore, store_set},
    south_provider_call::{
        CancellationDispositionV1, CommunityCallPolicyV1, CommunityCredentialResolverV1,
        IneligibleV1, PrepareProviderCallErrorV1, PreparedCommunityProviderCallV1,
        PreparedProviderStreamResultV1, RequestBodyModeV1, StableProviderCallFailureV1,
        build_direct_reqwest_streaming_transport_v1, build_direct_reqwest_transport_v1,
        execute_prepared_provider_call_v1, map_failure_v1, map_stream_read_failure_v1,
        open_prepared_provider_stream_v1, prepare_provider_call_v1, prepare_provider_stream_v1,
    },
};
use south_contracts::{
    BufferedHttpResponseV1, ContractErrorV1, CredentialSlotV1, MAX_JSON_REQUEST_BODY_BYTES,
    MAX_PROVIDER_QUOTA_METADATA_VALUE_BYTES, MAX_STREAM_ERROR_BODY_BYTES, PreparationErrorV1,
    ProviderQuotaMetadataFieldV1, ProviderQuotaMetadataV1, StreamChunkV1, StreamReadErrorV1,
    StreamRejectedV1, StreamingResponseHeadV1, TransportErrorV1,
};
use south_core::{
    AsyncHttpTransport, AsyncStreamingTransport, CredentialResolutionFuture, CredentialResolver,
    OpenedByteStreamV1, PreparedHttpRequestV1, ProviderCallErrorV1, SecretValue,
    StreamByteSourceV1, StreamChunkFutureV1, StreamOpenErrorV1, StreamingOpenFutureV1,
    TransportFuture,
};
use south_provider_conformance::{
    FAKE_BEARER_SECRET_V1, FAKE_HEADER_SECRET_V1, HeaderAuthFixtureV1, HeaderAuthUpstreamV1,
    PROVIDER_CALL_CONFORMANCE_DEADLINE_OFFSET_V1, PROVIDER_STREAM_CONFORMANCE_DEADLINE_OFFSET_V1,
    PROVIDER_STREAM_CONFORMANCE_IDLE_TIMEOUT_V1, ProviderCallControlV1, ProviderCallFailureCodeV1,
    ProviderCallFixtureV1, ProviderCallInputV1, ProviderCallUpstreamV1,
    ProviderQuotaMetadataFixtureV1, ProviderQuotaMetadataUpstreamV1, ProviderStreamControlV1,
    ProviderStreamFixtureV1, ProviderStreamRawHeadV1, ProviderStreamTerminalV1,
    ProviderStreamUpstreamV1,
};
use south_testkit::{
    AssembledExecutionFutureV1, AssembledHeaderAuthExecutionFutureV1,
    AssembledHeaderAuthExecutorV1, AssembledProviderCallExecutorV1,
    AssembledProviderQuotaMetadataExecutionFutureV1, AssembledProviderQuotaMetadataExecutorV1,
    AssembledProviderStreamExecutorV1, AssembledStreamExecutionFutureV1, HeaderAuthEvidenceV1,
    HeaderAuthObservationV1, ProviderCallEvidenceV1, ProviderCallObservationV1,
    ProviderQuotaMetadataEvidenceV1, ProviderQuotaMetadataObservationV1, ProviderStreamEvidenceV1,
    ProviderStreamObservationV1, run_header_auth_conformance_v1, run_provider_call_conformance_v1,
    run_provider_quota_metadata_conformance_v1, run_provider_stream_conformance_v1,
};
use std::{
    future::pending,
    io::{Read, Write},
    net::TcpListener,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    thread,
    time::Duration,
};
use token_station_protocol::{
    Auth, ErrorCode, HttpMethod, HttpRequestDescriptor, ProviderConfig, ProviderEndpoint,
    SafeHeaders, SecretRef,
};
use tokio::sync::{Notify, oneshot};
use tokio_util::sync::CancellationToken;
/// What a component that carries a bearer token declares.
fn bearer_arms() -> BTreeSet<AuthArmV1> {
    BTreeSet::from([AuthArmV1::Bearer])
}

/// What a component that carries a sanctioned secret header declares — the
/// Anthropic package's shape.
fn header_secret_arms() -> BTreeSet<AuthArmV1> {
    BTreeSet::from([AuthArmV1::HeaderSecret])
}

/// The OpenAI-compatible package's shape: it serves both the bearer dialects
/// and Azure's `api-key`, and says so. What used to be a "cumulative" host mode
/// is now just a component declaring two arms.
fn both_arms() -> BTreeSet<AuthArmV1> {
    BTreeSet::from([AuthArmV1::Bearer, AuthArmV1::HeaderSecret])
}

fn eligible_policy_with(arms: BTreeSet<AuthArmV1>) -> CommunityCallPolicyV1 {
    CommunityCallPolicyV1::new(
        ApiDialect::Translated,
        EgressMode::Direct,
        RequestBodyModeV1::Buffered,
        arms,
    )
}

fn eligible_streaming_policy_with(arms: BTreeSet<AuthArmV1>) -> CommunityCallPolicyV1 {
    CommunityCallPolicyV1::new(
        ApiDialect::Translated,
        EgressMode::Direct,
        RequestBodyModeV1::Streaming,
        arms,
    )
}

fn eligible_policy() -> CommunityCallPolicyV1 {
    CommunityCallPolicyV1::new(
        ApiDialect::Translated,
        EgressMode::Direct,
        RequestBodyModeV1::Buffered,
        bearer_arms(),
    )
}

fn eligible_streaming_policy() -> CommunityCallPolicyV1 {
    CommunityCallPolicyV1::new(
        ApiDialect::Translated,
        EgressMode::Direct,
        RequestBodyModeV1::Streaming,
        bearer_arms(),
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

fn azure_provider_config() -> ProviderConfig {
    let mut config = ProviderConfig::new(
        "azure-openai-v1",
        ProviderEndpoint::try_new("https://fixture.openai.azure.com/openai/v1")
            .expect("test endpoint is valid"),
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

fn header_descriptor(name: &str, slot: &str) -> HttpRequestDescriptor {
    let mut descriptor = descriptor();
    descriptor.auth = Some(
        Auth::header(name, SecretRef::new(slot))
            .expect("the test header must be covered by host redaction"),
    );
    descriptor
}

#[test]
fn eligible_descriptor_projects_into_the_south_contract() {
    let prepared = prepare_provider_call_v1(
        &eligible_policy(),
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
fn eligible_streaming_descriptor_projects_into_the_same_bounded_contract() {
    let prepared = prepare_provider_stream_v1(
        &eligible_streaming_policy(),
        &provider_config(),
        &auth_config(),
        &descriptor(),
    )
    .expect("eligible streaming descriptor should project");

    assert_eq!(prepared.relative_path(), "chat/completions");
    assert_eq!(prepared.credential_slot(), "provider_api_key");
    assert_eq!(prepared.header_count(), 2);
    assert_eq!(prepared.body(), r#"{"model":"test"}"#);
}

#[test]
fn header_auth_requires_an_independent_explicit_capability() {
    let error = prepare_provider_call_v1(
        &eligible_policy(),
        &provider_config(),
        &auth_config(),
        &header_descriptor("x-api-key", "provider_api_key"),
    )
    .expect_err("the existing South opt-in must remain Bearer-only");

    assert_eq!(
        error,
        PrepareProviderCallErrorV1::Ineligible(IneligibleV1::Auth)
    );
}

#[test]
fn azure_header_auth_requires_the_new_production_capability() {
    let mut request = header_descriptor("api-key", "provider_api_key");
    request.url = "https://fixture.openai.azure.com/openai/v1/chat/completions".to_owned();

    let old_mode = prepare_provider_call_v1(
        &eligible_policy(),
        &azure_provider_config(),
        &auth_config(),
        &request,
    )
    .expect_err("a component declaring only bearer must not be handed a header secret");
    // The reason moved with the judgement: it is no longer "this dialect is not
    // on the list" but "this component does not carry that auth shape".
    assert_eq!(
        old_mode,
        PrepareProviderCallErrorV1::Ineligible(IneligibleV1::Auth)
    );

    let prepared = prepare_provider_call_v1(
        &eligible_policy_with(header_secret_arms()),
        &azure_provider_config(),
        &auth_config(),
        &request,
    )
    .expect("the exact Azure dialect and api-key pair must be eligible");
    assert_eq!(prepared.relative_path(), "chat/completions");
    assert_eq!(prepared.credential_slot(), "provider_api_key");
}

#[test]
fn a_component_declaring_both_arms_still_carries_a_bearer() {
    let prepared = prepare_provider_call_v1(
        &eligible_policy_with(both_arms()),
        &provider_config(),
        &auth_config(),
        &descriptor(),
    )
    .expect("the cumulative Header Auth mode must retain OpenAI Bearer eligibility");

    assert_eq!(prepared.relative_path(), "chat/completions");
    assert_eq!(prepared.credential_slot(), "provider_api_key");
}

#[test]
fn production_header_auth_does_not_open_the_full_compatibility_catalog() {
    let mut request = header_descriptor("x-api-key", "provider_api_key");
    request.url = "https://fixture.openai.azure.com/openai/v1/chat/completions".to_owned();

    // A sanctioned header on a component that declares the arm is eligible,
    // whatever the dialect is called: `x-api-key` is in the catalogue, so the
    // per-dialect narrowing that used to reject it here is gone on purpose.
    prepare_provider_call_v1(
        &eligible_policy_with(header_secret_arms()),
        &azure_provider_config(),
        &auth_config(),
        &request,
    )
    .expect("a sanctioned header the component declares it carries is eligible");

    // What still bites is the catalogue, and it bites earlier than the transport:
    // an arbitrary header name is not a credential header this host knows how to
    // redact, so `Auth::header` refuses to build the descriptor at all. The
    // transport never sees the shape it would have had to reject.
    assert!(
        Auth::header("x-invented-key", SecretRef::new("provider_api_key")).is_err(),
        "a header outside the sanctioned catalogue must not become an Auth at all"
    );

    // "Bearer-only" was a host-side restriction, not a property of the package:
    // the shipped OpenAI-compatible component declares both arms and serves
    // Azure's `api-key` with them. What refuses a header now is a component that
    // says it carries only bearers.
    let openai_header = prepare_provider_call_v1(
        &eligible_policy(),
        &provider_config(),
        &auth_config(),
        &header_descriptor("api-key", "provider_api_key"),
    )
    .expect_err("a bearer-only component must not be handed a header secret");
    assert_eq!(
        openai_header,
        PrepareProviderCallErrorV1::Ineligible(IneligibleV1::Auth)
    );

    let mut azure_bearer = descriptor();
    azure_bearer.url = "https://fixture.openai.azure.com/openai/v1/chat/completions".to_owned();
    let azure_bearer = prepare_provider_call_v1(
        &eligible_policy_with(header_secret_arms()),
        &azure_provider_config(),
        &auth_config(),
        &azure_bearer,
    )
    .expect_err("the Azure dialect must not silently fall back to Bearer presentation");
    assert_eq!(
        azure_bearer,
        PrepareProviderCallErrorV1::Ineligible(IneligibleV1::Auth)
    );
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn explicit_header_auth_maps_only_the_five_south_sanctioned_names() {
    for (input_name, canonical_name) in [
        ("api-key", "api-key"),
        ("X-API-Key", "x-api-key"),
        ("x-goog-api-key", "x-goog-api-key"),
        ("xi-api-key", "xi-api-key"),
        ("ocp-apim-subscription-key", "ocp-apim-subscription-key"),
    ] {
        let prepared = prepare_provider_call_v1(
            &eligible_policy_with(header_secret_arms()),
            &provider_config(),
            &auth_config(),
            &header_descriptor(input_name, "provider_api_key"),
        )
        .expect("a sanctioned header must project when the capability is explicit");
        let resolver = ImmediateResolver {
            calls: AtomicUsize::new(0),
        };
        let transport = HeaderInspectingTransport {
            calls: AtomicUsize::new(0),
            expected_name: canonical_name,
        };

        execute_prepared_provider_call_v1(
            &prepared,
            &resolver,
            &transport,
            tokio::time::Instant::now() + Duration::from_secs(30),
            &CancellationToken::new(),
        )
        .await
        .expect("the projected header-secret request must execute");

        assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
        assert_eq!(transport.calls.load(Ordering::SeqCst), 1);
    }

    let error = prepare_provider_call_v1(
        &eligible_policy_with(header_secret_arms()),
        &provider_config(),
        &auth_config(),
        &header_descriptor("cookie", "provider_api_key"),
    )
    .expect_err("a credential header outside South's closed set must fail locally");
    assert_eq!(
        error,
        PrepareProviderCallErrorV1::Ineligible(IneligibleV1::Auth)
    );
}

#[test]
fn unsupported_shapes_are_closed_host_local_reasons() {
    let cases = [
        (
            CommunityCallPolicyV1::new(
                ApiDialect::AnthropicNative,
                EgressMode::Direct,
                RequestBodyModeV1::Buffered,
                bearer_arms(),
            ),
            IneligibleV1::ApiDialect,
        ),
        (
            CommunityCallPolicyV1::new(
                ApiDialect::Translated,
                EgressMode::Http,
                RequestBodyModeV1::Buffered,
                bearer_arms(),
            ),
            IneligibleV1::Egress,
        ),
        (
            CommunityCallPolicyV1::new(
                ApiDialect::Translated,
                EgressMode::Direct,
                RequestBodyModeV1::Streaming,
                bearer_arms(),
            ),
            IneligibleV1::Streaming,
        ),
    ];

    for (policy, expected) in cases {
        let error =
            prepare_provider_call_v1(&policy, &provider_config(), &auth_config(), &descriptor())
                .expect_err("unsupported policy must not project");
        assert_eq!(error, PrepareProviderCallErrorV1::Ineligible(expected));
    }
}

#[test]
fn unsupported_descriptor_and_secret_shapes_are_closed_host_local_reasons() {
    // The dialect's name no longer decides anything: an Anthropic upstream whose
    // component declares the bearer arm projects like any other. What is refused
    // below is a shape no admitted component claims to carry.
    let mut anthropic = provider_config();
    anthropic.provider = "anthropic".to_owned();
    prepare_provider_call_v1(
        &eligible_policy(),
        &anthropic,
        &auth_config(),
        &descriptor(),
    )
    .expect("a dialect name is not an eligibility fact once admission has run");

    let mut get = descriptor();
    get.method = HttpMethod::Get;
    let error =
        prepare_provider_call_v1(&eligible_policy(), &provider_config(), &auth_config(), &get)
            .expect_err("GET must not project");
    assert_eq!(
        error,
        PrepareProviderCallErrorV1::Ineligible(IneligibleV1::Method)
    );

    let mut no_body = descriptor();
    no_body.body = None;
    let error = prepare_provider_call_v1(
        &eligible_policy(),
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
        &eligible_policy(),
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
        &eligible_policy(),
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
            &eligible_policy(),
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
        &eligible_policy(),
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
        &eligible_policy(),
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
        &eligible_policy(),
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
        &eligible_policy(),
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
        &eligible_policy(),
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

    let prepared = prepare_provider_call_v1(&eligible_policy(), &provider, &auth, &descriptor)
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

struct HeaderInspectingTransport<'name> {
    calls: AtomicUsize,
    expected_name: &'name str,
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
        assert_eq!(
            request.auth_headers().collect::<Vec<_>>(),
            vec![("authorization", b"Bearer synthetic-test-secret".as_slice())]
        );
        Box::pin(async {
            let quota_metadata = ProviderQuotaMetadataV1::try_from_iter([
                (
                    ProviderQuotaMetadataFieldV1::XRateLimitLimitTokens,
                    "1000".to_owned(),
                ),
                (
                    ProviderQuotaMetadataFieldV1::XRateLimitRemainingTokens,
                    "900".to_owned(),
                ),
                (
                    ProviderQuotaMetadataFieldV1::XRateLimitResetTokens,
                    "10s".to_owned(),
                ),
                (
                    ProviderQuotaMetadataFieldV1::AnthropicRateLimitTokensLimit,
                    "2000".to_owned(),
                ),
                (
                    ProviderQuotaMetadataFieldV1::AnthropicRateLimitTokensRemaining,
                    "1500".to_owned(),
                ),
                (
                    ProviderQuotaMetadataFieldV1::AnthropicRateLimitTokensReset,
                    "20s".to_owned(),
                ),
                (
                    ProviderQuotaMetadataFieldV1::AnthropicRateLimitUnifiedLimit,
                    "3000".to_owned(),
                ),
                (
                    ProviderQuotaMetadataFieldV1::AnthropicRateLimitUnifiedRemaining,
                    "2500".to_owned(),
                ),
                (
                    ProviderQuotaMetadataFieldV1::AnthropicRateLimitUnifiedReset,
                    "1970-01-01T00:00:30Z".to_owned(),
                ),
            ])?;
            BufferedHttpResponseV1::try_from_parts_with_provider_quota_metadata(
                201_u16.try_into().expect("test status is valid"),
                br#"{"ok":true}"#.to_vec(),
                Some("application/json".to_owned()),
                Some("2".to_owned()),
                quota_metadata,
            )
        })
    }
}

impl AsyncHttpTransport for HeaderInspectingTransport<'_> {
    fn execute<'a>(
        &'a self,
        request: &'a PreparedHttpRequestV1<'_>,
        _remaining_timeout: Duration,
    ) -> TransportFuture<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        assert_eq!(
            request.auth_headers().collect::<Vec<_>>(),
            vec![(self.expected_name, b"synthetic-test-secret".as_slice())]
        );
        Box::pin(async {
            BufferedHttpResponseV1::try_from_parts(
                204_u16.try_into().expect("test status is valid"),
                Vec::new(),
                None,
                None,
            )
        })
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn assembled_execution_uses_real_south_core_and_projects_the_response() {
    let prepared = prepare_provider_call_v1(
        &eligible_policy(),
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
    assert_eq!(
        response.headers,
        [
            (
                "anthropic-ratelimit-tokens-limit".to_owned(),
                "2000".to_owned()
            ),
            (
                "anthropic-ratelimit-tokens-remaining".to_owned(),
                "1500".to_owned(),
            ),
            (
                "anthropic-ratelimit-tokens-reset".to_owned(),
                "20s".to_owned()
            ),
            (
                "anthropic-ratelimit-unified-limit".to_owned(),
                "3000".to_owned()
            ),
            (
                "anthropic-ratelimit-unified-remaining".to_owned(),
                "2500".to_owned(),
            ),
            (
                "anthropic-ratelimit-unified-reset".to_owned(),
                "1970-01-01T00:00:30Z".to_owned(),
            ),
            ("content-type".to_owned(), "application/json".to_owned()),
            ("retry-after".to_owned(), "2".to_owned()),
            ("x-ratelimit-limit-tokens".to_owned(), "1000".to_owned()),
            ("x-ratelimit-remaining-tokens".to_owned(), "900".to_owned()),
            ("x-ratelimit-reset-tokens".to_owned(), "10s".to_owned()),
        ]
        .into_iter()
        .collect()
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
        (ContractErrorV1::InvalidQueryValue, ErrorCode::Internal, 500),
        (
            ContractErrorV1::DuplicateQueryParameter,
            ErrorCode::Internal,
            500,
        ),
        (ContractErrorV1::EmptyQuery, ErrorCode::Internal, 500),
        (ContractErrorV1::QueryTooLarge, ErrorCode::Internal, 500),
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

#[test]
fn streaming_rejection_fails_closed_in_the_buffered_adapter() {
    let head = StreamingResponseHeadV1::try_from_parts(
        "503".parse().expect("fixture status is valid"),
        Some("application/json".to_owned()),
        None,
    )
    .expect("fixture streaming head is valid");
    let rejection = StreamRejectedV1::new(head, b"streaming-body-sentinel".to_vec());

    let mapped = map_failure_v1(
        StableProviderCallFailureV1::Provider(ProviderCallErrorV1::Rejected(rejection)),
        CancellationDispositionV1::Deadline,
    );

    assert_eq!(
        (mapped.code, mapped.http_status),
        (ErrorCode::ProviderProtocolError, 502)
    );
    assert!(!mapped.message.contains("streaming-body-sentinel"));
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
        &eligible_policy(),
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

#[test]
fn direct_streaming_transport_requires_connect_and_idle_guards_without_a_total_cap() {
    let transport = build_direct_reqwest_streaming_transport_v1(
        None,
        Duration::from_secs(5),
        Duration::from_secs(5),
    )
    .expect("valid explicit guards should build a dedicated streaming transport");
    assert_eq!(
        format!("{transport:?}"),
        "ReqwestStreamingTransportV1 { .. }"
    );

    let failure =
        build_direct_reqwest_streaming_transport_v1(None, Duration::from_secs(1), Duration::ZERO)
            .expect_err("zero idle timeout must fail closed");
    assert_eq!(
        failure,
        StableProviderCallFailureV1::Provider(ProviderCallErrorV1::Transport(
            TransportErrorV1::ClientBuildFailed,
        ))
    );
}

#[tokio::test(flavor = "current_thread")]
async fn real_streaming_adapter_opens_at_headers_and_pulls_the_exact_body_once() {
    let body = b"data: {\"delta\":\"hello\"}\n\ndata: [DONE]\n\n";
    let response = format!(
        concat!(
            "HTTP/1.1 200 OK\r\n",
            "content-type: text/event-stream\r\n",
            "x-ratelimit-limit-tokens: 1000\r\n",
            "content-length: {}\r\n",
            "connection: close\r\n",
            "\r\n"
        ),
        body.len()
    )
    .into_bytes();
    let mut response = response;
    response.extend_from_slice(body);
    let (base_url, server) = start_quota_loopback(response);
    let mut provider = ProviderConfig::new(
        "openai-compatible",
        ProviderEndpoint::try_new(&base_url).expect("loopback endpoint is valid"),
    );
    provider.auth = Some(SecretRef::new("provider_api_key"));
    let mut request = descriptor();
    request.url = format!("{base_url}/chat/completions");
    let prepared = prepare_provider_stream_v1(
        &eligible_streaming_policy(),
        &provider,
        &auth_config(),
        &request,
    )
    .expect("streaming request projects");
    let resolver = ImmediateResolver {
        calls: AtomicUsize::new(0),
    };
    let transport = build_direct_reqwest_streaming_transport_v1(
        None,
        Duration::from_secs(2),
        Duration::from_secs(2),
    )
    .expect("streaming transport builds");

    let mut stream = match open_prepared_provider_stream_v1(
        &prepared,
        &resolver,
        &transport,
        Some(tokio::time::Instant::now() + Duration::from_secs(5)),
        &CancellationToken::new(),
    )
    .await
    .expect("stream opens")
    {
        PreparedProviderStreamResultV1::Opened(stream) => stream,
        PreparedProviderStreamResultV1::Rejected(_) => panic!("2xx must open a stream"),
    };
    assert_eq!(stream.head().status().as_u16(), 200);
    assert_eq!(stream.head().content_type(), Some("text/event-stream"));
    assert_eq!(
        stream
            .head()
            .provider_quota_metadata()
            .value(ProviderQuotaMetadataFieldV1::XRateLimitLimitTokens),
        Some("1000")
    );
    let mut delivered = Vec::new();
    while let Some(chunk) = stream.next_chunk().await {
        delivered.extend_from_slice(chunk.expect("loopback chunk is valid").as_bytes());
    }
    assert_eq!(delivered, body);
    assert_eq!(resolver.calls.load(Ordering::SeqCst), 1);
    assert_eq!(server.join().expect("loopback joins"), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn real_reqwest_transports_inject_only_the_sanctioned_header_secret() {
    let buffered_request = run_real_buffered_header_auth().await;
    assert_header_auth_wire(&buffered_request, "x-api-key");

    let streaming_request = run_real_streaming_header_auth().await;
    assert_header_auth_wire(&streaming_request, "x-goog-api-key");
}

async fn run_real_buffered_header_auth() -> String {
    let buffered_body = br#"{"ok":true}"#;
    let mut buffered_response = format!(
        concat!(
            "HTTP/1.1 200 OK\r\n",
            "content-type: application/json\r\n",
            "content-length: {}\r\n",
            "connection: close\r\n",
            "\r\n"
        ),
        buffered_body.len()
    )
    .into_bytes();
    buffered_response.extend_from_slice(buffered_body);
    let (base_url, buffered_server) = start_header_auth_loopback(buffered_response);
    let mut provider = ProviderConfig::new(
        "openai-compatible",
        ProviderEndpoint::try_new(&base_url).expect("loopback endpoint is valid"),
    );
    provider.auth = Some(SecretRef::new("provider_api_key"));
    let mut request = header_descriptor("x-api-key", "provider_api_key");
    request.url = format!("{base_url}/chat/completions");
    let prepared = prepare_provider_call_v1(
        &eligible_policy_with(header_secret_arms()),
        &provider,
        &auth_config(),
        &request,
    )
    .expect("buffered Header Auth request projects");
    let resolver = ImmediateResolver {
        calls: AtomicUsize::new(0),
    };
    let transport = build_direct_reqwest_transport_v1(
        Duration::from_secs(5),
        Duration::from_secs(2),
        Duration::from_secs(2),
    )
    .expect("buffered transport builds");
    execute_prepared_provider_call_v1(
        &prepared,
        &resolver,
        &transport,
        tokio::time::Instant::now() + Duration::from_secs(5),
        &CancellationToken::new(),
    )
    .await
    .expect("buffered Header Auth request succeeds");
    buffered_server.join().expect("buffered loopback joins")
}

async fn run_real_streaming_header_auth() -> String {
    let streaming_body = b"data: [DONE]\n\n";
    let mut streaming_response = format!(
        concat!(
            "HTTP/1.1 200 OK\r\n",
            "content-type: text/event-stream\r\n",
            "content-length: {}\r\n",
            "connection: close\r\n",
            "\r\n"
        ),
        streaming_body.len()
    )
    .into_bytes();
    streaming_response.extend_from_slice(streaming_body);
    let (base_url, streaming_server) = start_header_auth_loopback(streaming_response);
    let mut provider = ProviderConfig::new(
        "openai-compatible",
        ProviderEndpoint::try_new(&base_url).expect("loopback endpoint is valid"),
    );
    provider.auth = Some(SecretRef::new("provider_api_key"));
    let mut request = header_descriptor("x-goog-api-key", "provider_api_key");
    request.url = format!("{base_url}/chat/completions");
    let prepared = prepare_provider_stream_v1(
        &eligible_streaming_policy_with(header_secret_arms()),
        &provider,
        &auth_config(),
        &request,
    )
    .expect("streaming Header Auth request projects");
    let resolver = ImmediateResolver {
        calls: AtomicUsize::new(0),
    };
    let transport = build_direct_reqwest_streaming_transport_v1(
        None,
        Duration::from_secs(2),
        Duration::from_secs(2),
    )
    .expect("streaming transport builds");
    let mut stream = match open_prepared_provider_stream_v1(
        &prepared,
        &resolver,
        &transport,
        Some(tokio::time::Instant::now() + Duration::from_secs(5)),
        &CancellationToken::new(),
    )
    .await
    .expect("streaming Header Auth request opens")
    {
        PreparedProviderStreamResultV1::Opened(stream) => stream,
        PreparedProviderStreamResultV1::Rejected(_) => panic!("2xx must open a stream"),
    };
    let mut delivered = Vec::new();
    while let Some(chunk) = stream.next_chunk().await {
        delivered.extend_from_slice(chunk.expect("streaming chunk is valid").as_bytes());
    }
    assert_eq!(delivered, streaming_body);
    streaming_server.join().expect("streaming loopback joins")
}

fn assert_header_auth_wire(request: &str, expected_name: &str) {
    let expected_value = "synthetic-test-secret";
    let actual_value = request.lines().find_map(|line| {
        let (name, value) = line.trim_end_matches('\r').split_once(':')?;
        name.eq_ignore_ascii_case(expected_name)
            .then_some(value.trim_start())
    });
    assert_eq!(
        actual_value,
        Some(expected_value),
        "the sanctioned header must carry the exact synthetic secret"
    );
    assert!(
        !request.lines().any(|line| {
            line.split_once(':')
                .is_some_and(|(name, _)| name.eq_ignore_ascii_case("authorization"))
        }),
        "a Header Auth request must not also carry Authorization"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn real_streaming_rejections_are_bounded_projected_and_never_become_live_streams() {
    let body = br#"{"error":{"message":"rate limited"}}"#;
    let mut response = format!(
        concat!(
            "HTTP/1.1 429 Too Many Requests\r\n",
            "content-type: application/json\r\n",
            "retry-after: 2\r\n",
            "content-length: {}\r\n",
            "connection: close\r\n",
            "\r\n"
        ),
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    let (base_url, server) = start_quota_loopback(response);
    let mut provider = ProviderConfig::new(
        "openai-compatible",
        ProviderEndpoint::try_new(&base_url).expect("loopback endpoint is valid"),
    );
    provider.auth = Some(SecretRef::new("provider_api_key"));
    let mut request = descriptor();
    request.url = format!("{base_url}/chat/completions");
    let prepared = prepare_provider_stream_v1(
        &eligible_streaming_policy(),
        &provider,
        &auth_config(),
        &request,
    )
    .expect("streaming request projects");
    let resolver = ImmediateResolver {
        calls: AtomicUsize::new(0),
    };
    let transport = build_direct_reqwest_streaming_transport_v1(
        None,
        Duration::from_secs(2),
        Duration::from_secs(2),
    )
    .expect("streaming transport builds");

    let rejected = open_prepared_provider_stream_v1(
        &prepared,
        &resolver,
        &transport,
        Some(tokio::time::Instant::now() + Duration::from_secs(5)),
        &CancellationToken::new(),
    )
    .await
    .expect("bounded rejection is a valid open outcome");
    let PreparedProviderStreamResultV1::Rejected(rejected) = rejected else {
        panic!("non-2xx must not create a live stream");
    };
    assert_eq!(rejected.status, 429);
    assert_eq!(rejected.body.as_bytes(), body);
    assert_eq!(
        rejected.headers.get("retry-after").map(String::as_str),
        Some("2")
    );
    assert_eq!(server.join().expect("loopback joins"), 1);
}

#[tokio::test(flavor = "current_thread")]
async fn truncated_streaming_rejection_preserves_the_upstream_status() {
    let mut body = vec![b'a'; MAX_STREAM_ERROR_BODY_BYTES - 1];
    body.extend_from_slice("étail".as_bytes());
    let mut response = format!(
        concat!(
            "HTTP/1.1 429 Too Many Requests\r\n",
            "content-type: application/json\r\n",
            "content-length: {}\r\n",
            "connection: close\r\n",
            "\r\n"
        ),
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(&body);
    let (base_url, server) = start_quota_loopback(response);
    let mut provider = ProviderConfig::new(
        "openai-compatible",
        ProviderEndpoint::try_new(&base_url).expect("loopback endpoint is valid"),
    );
    provider.auth = Some(SecretRef::new("provider_api_key"));
    let mut request = descriptor();
    request.url = format!("{base_url}/chat/completions");
    let prepared = prepare_provider_stream_v1(
        &eligible_streaming_policy(),
        &provider,
        &auth_config(),
        &request,
    )
    .expect("streaming request projects");
    let resolver = ImmediateResolver {
        calls: AtomicUsize::new(0),
    };
    let transport = build_direct_reqwest_streaming_transport_v1(
        None,
        Duration::from_secs(2),
        Duration::from_secs(2),
    )
    .expect("streaming transport builds");

    let result = open_prepared_provider_stream_v1(
        &prepared,
        &resolver,
        &transport,
        Some(tokio::time::Instant::now() + Duration::from_secs(5)),
        &CancellationToken::new(),
    )
    .await
    .expect("body decoding cannot replace the authoritative rejection status");
    let PreparedProviderStreamResultV1::Rejected(rejected) = result else {
        panic!("non-2xx must not create a live stream");
    };
    assert_eq!(rejected.status, 429);
    assert!(rejected.body.ends_with('\u{fffd}'));
    assert_eq!(server.join().expect("loopback joins"), 1);
}

#[test]
fn every_midstream_failure_maps_to_the_existing_closed_host_catalog() {
    let cases = [
        (
            StreamReadErrorV1::StreamReadFailed,
            CancellationDispositionV1::ClientDisconnected,
            ErrorCode::TransportTruncated,
            502,
        ),
        (
            StreamReadErrorV1::StreamIdleTimeout,
            CancellationDispositionV1::ClientDisconnected,
            ErrorCode::Timeout,
            504,
        ),
        (
            StreamReadErrorV1::StreamDeadlineExceeded,
            CancellationDispositionV1::Deadline,
            ErrorCode::Timeout,
            504,
        ),
        (
            StreamReadErrorV1::StreamCancelled,
            CancellationDispositionV1::ClientDisconnected,
            ErrorCode::Internal,
            499,
        ),
        (
            StreamReadErrorV1::StreamCancelled,
            CancellationDispositionV1::ServerDrain,
            ErrorCode::UpstreamUnavailable,
            503,
        ),
        (
            StreamReadErrorV1::ChunkNotDeliverable,
            CancellationDispositionV1::ClientDisconnected,
            ErrorCode::ProviderProtocolError,
            502,
        ),
    ];

    for (failure, disposition, expected_code, expected_status) in cases {
        let mapped = map_stream_read_failure_v1(failure, disposition);
        assert_eq!(
            (mapped.code, mapped.http_status),
            (expected_code, expected_status)
        );
    }
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn community_streaming_adapter_passes_the_public_south_suite() {
    let executor = CommunityStreamConformanceExecutorV1::new();
    let run = run_provider_stream_conformance_v1(&executor);
    let drive_clock = async {
        executor.idle_stall_started().await;
        tokio::time::advance(PROVIDER_STREAM_CONFORMANCE_IDLE_TIMEOUT_V1).await;
        executor.deadline_chunk_started().await;
        tokio::time::advance(PROVIDER_STREAM_CONFORMANCE_DEADLINE_OFFSET_V1).await;
    };
    let structured = async { tokio::join!(run, drive_clock) };
    let (report, ()) = tokio::time::timeout(Duration::from_secs(5), structured)
        .await
        .expect("structured conformance watchdog must not expire");
    let report = report.expect("community streaming adapter must pass the public suite");

    assert_eq!(report.suite_id(), "south.provider-stream.v1");
    assert_eq!(report.suite_version(), 1);
    assert_eq!(report.passed_case_ids().len(), 9);
}

#[tokio::test(flavor = "current_thread", start_paused = true)]
async fn community_adapter_passes_the_public_header_auth_suite() {
    let executor = CommunityHeaderAuthConformanceExecutorV1;
    let report = tokio::time::timeout(
        Duration::from_secs(5),
        run_header_auth_conformance_v1(&executor),
    )
    .await
    .expect("structured header-auth conformance watchdog must not expire")
    .expect("community adapter must pass the public Header Auth suite");

    assert_eq!(report.suite_id(), "south.header-auth.v1");
    assert_eq!(report.suite_version(), 1);
    assert_eq!(report.passed_case_ids().len(), 3);
}

struct CommunityHeaderAuthConformanceExecutorV1;

struct HeaderAuthProbe {
    resolver_calls: AtomicUsize,
    transport_calls: AtomicUsize,
    sanctioned_header_exact: AtomicBool,
    authorization_header_absent: AtomicBool,
}

impl HeaderAuthProbe {
    fn new() -> Self {
        Self {
            resolver_calls: AtomicUsize::new(0),
            transport_calls: AtomicUsize::new(0),
            sanctioned_header_exact: AtomicBool::new(false),
            authorization_header_absent: AtomicBool::new(true),
        }
    }

    fn inspect(&self, request: &PreparedHttpRequestV1<'_>, fixture: &HeaderAuthFixtureV1) {
        let mut auth_headers = request.auth_headers();
        let (name, value) = auth_headers
            .next()
            .expect("a header-auth request binds exactly one auth header");
        assert!(auth_headers.next().is_none());
        self.sanctioned_header_exact.store(
            name == fixture.secret_header().header_name()
                && value == FAKE_HEADER_SECRET_V1.as_bytes(),
            Ordering::SeqCst,
        );
        self.authorization_header_absent
            .store(name != "authorization", Ordering::SeqCst);
    }

    fn evidence(&self) -> HeaderAuthEvidenceV1 {
        HeaderAuthEvidenceV1::new(
            self.resolver_calls.load(Ordering::SeqCst),
            self.transport_calls.load(Ordering::SeqCst),
            self.sanctioned_header_exact.load(Ordering::SeqCst),
            self.authorization_header_absent.load(Ordering::SeqCst),
        )
    }
}

struct HeaderAuthConformanceResolver<'probe> {
    probe: &'probe HeaderAuthProbe,
}

impl CredentialResolver for HeaderAuthConformanceResolver<'_> {
    fn resolve<'a>(&'a self, _slot: &'a CredentialSlotV1) -> CredentialResolutionFuture<'a> {
        self.probe.resolver_calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(SecretValue::new(FAKE_HEADER_SECRET_V1.to_owned())) })
    }
}

struct HeaderAuthConformanceTransport<'fixture, 'probe> {
    fixture: &'fixture HeaderAuthFixtureV1,
    probe: &'probe HeaderAuthProbe,
}

impl AsyncHttpTransport for HeaderAuthConformanceTransport<'_, '_> {
    fn execute<'a>(
        &'a self,
        request: &'a PreparedHttpRequestV1<'_>,
        _remaining_timeout: Duration,
    ) -> TransportFuture<'a> {
        self.probe.transport_calls.fetch_add(1, Ordering::SeqCst);
        self.probe.inspect(request, self.fixture);
        let upstream = *self.fixture.upstream();
        Box::pin(async move {
            match upstream {
                HeaderAuthUpstreamV1::Response(raw) => BufferedHttpResponseV1::try_from_parts(
                    raw.status()
                        .try_into()
                        .map_err(|_| TransportErrorV1::ResponseMetadataInvalid)?,
                    raw.body().as_bytes().to_vec(),
                    raw.content_type().map(str::to_owned),
                    raw.retry_after().map(str::to_owned),
                ),
                HeaderAuthUpstreamV1::Stream(_) | HeaderAuthUpstreamV1::NotReached => {
                    Err(TransportErrorV1::RequestFailed)
                }
            }
        })
    }
}

struct HeaderAuthConformanceStreamingTransport<'fixture, 'probe> {
    fixture: &'fixture HeaderAuthFixtureV1,
    probe: &'probe HeaderAuthProbe,
}

impl AsyncStreamingTransport for HeaderAuthConformanceStreamingTransport<'_, '_> {
    fn open<'a>(&'a self, request: &'a PreparedHttpRequestV1<'_>) -> StreamingOpenFutureV1<'a> {
        self.probe.transport_calls.fetch_add(1, Ordering::SeqCst);
        self.probe.inspect(request, self.fixture);
        let upstream = *self.fixture.upstream();
        Box::pin(async move {
            match upstream {
                HeaderAuthUpstreamV1::Stream(raw) => {
                    let source = ConformanceStreamSource {
                        chunks: raw.chunks(),
                        next_index: 0,
                        terminal: raw.terminal(),
                        stall_started: Arc::new(Notify::new()),
                        dropped: Arc::new(AtomicBool::new(false)),
                    };
                    OpenedByteStreamV1::try_new(
                        conformance_stream_head(raw.head())?,
                        Box::new(source),
                    )
                    .map_err(StreamOpenErrorV1::Transport)
                }
                HeaderAuthUpstreamV1::Response(_) | HeaderAuthUpstreamV1::NotReached => Err(
                    StreamOpenErrorV1::Transport(TransportErrorV1::RequestFailed),
                ),
            }
        })
    }
}

fn prepare_header_auth_conformance_fixture(
    fixture: &HeaderAuthFixtureV1,
    streaming: bool,
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
    descriptor.auth = Some(
        Auth::header(
            fixture.secret_header().header_name(),
            SecretRef::new(input.requested_credential_slot()),
        )
        .expect("canonical Header Auth fixture uses a host-redacted name"),
    );
    let policy = if streaming {
        eligible_streaming_policy_with(header_secret_arms())
    } else {
        eligible_policy_with(header_secret_arms())
    };

    if streaming {
        prepare_provider_stream_v1(&policy, &provider, &auth, &descriptor)
    } else {
        prepare_provider_call_v1(&policy, &provider, &auth, &descriptor)
    }
}

impl CommunityHeaderAuthConformanceExecutorV1 {
    async fn execute_community_case(
        &self,
        fixture: &HeaderAuthFixtureV1,
    ) -> HeaderAuthObservationV1 {
        let probe = HeaderAuthProbe::new();
        let streaming = matches!(fixture.upstream(), HeaderAuthUpstreamV1::Stream(_));
        let prepared = match prepare_header_auth_conformance_fixture(fixture, streaming) {
            Ok(prepared) => prepared,
            Err(error) => {
                return HeaderAuthObservationV1::failure(
                    map_prepare_failure_code(error),
                    probe.evidence(),
                );
            }
        };
        let resolver = HeaderAuthConformanceResolver { probe: &probe };
        let cancellation = CancellationToken::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(30);

        if streaming {
            let transport = HeaderAuthConformanceStreamingTransport {
                fixture,
                probe: &probe,
            };
            return match open_prepared_provider_stream_v1(
                &prepared,
                &resolver,
                &transport,
                Some(deadline),
                &cancellation,
            )
            .await
            {
                Ok(PreparedProviderStreamResultV1::Opened(mut stream)) => {
                    let head = stream.head().clone();
                    let mut chunks = Vec::new();
                    let mut failed = false;
                    while let Some(next) = stream.next_chunk().await {
                        if let Ok(chunk) = next {
                            chunks.push(chunk);
                        } else {
                            failed = true;
                            break;
                        }
                    }
                    if failed {
                        HeaderAuthObservationV1::failure(
                            ProviderCallFailureCodeV1::RequestFailed,
                            probe.evidence(),
                        )
                    } else {
                        HeaderAuthObservationV1::opened(head, chunks, probe.evidence())
                    }
                }
                Ok(PreparedProviderStreamResultV1::Rejected(_)) => {
                    HeaderAuthObservationV1::failure(
                        ProviderCallFailureCodeV1::RequestFailed,
                        probe.evidence(),
                    )
                }
                Err(error) => HeaderAuthObservationV1::failure(
                    map_stable_failure_code(&error),
                    probe.evidence(),
                ),
            };
        }

        let transport = HeaderAuthConformanceTransport {
            fixture,
            probe: &probe,
        };
        match execute_prepared_provider_call_v1(
            &prepared,
            &resolver,
            &transport,
            deadline,
            &cancellation,
        )
        .await
        {
            Ok(response) => {
                let bounded = BufferedHttpResponseV1::try_from_parts(
                    response
                        .status
                        .try_into()
                        .expect("canonical response status is valid"),
                    response.body.into_bytes(),
                    response.headers.get("content-type").cloned(),
                    response.headers.get("retry-after").cloned(),
                )
                .expect("adapter response remains inside the South response contract");
                HeaderAuthObservationV1::response(bounded, probe.evidence())
            }
            Err(error) => {
                HeaderAuthObservationV1::failure(map_stable_failure_code(&error), probe.evidence())
            }
        }
    }
}

impl AssembledHeaderAuthExecutorV1 for CommunityHeaderAuthConformanceExecutorV1 {
    fn execute_case<'a>(
        &'a self,
        fixture: &'a HeaderAuthFixtureV1,
    ) -> AssembledHeaderAuthExecutionFutureV1<'a> {
        Box::pin(async move { self.execute_community_case(fixture).await })
    }
}

struct CommunityStreamConformanceExecutorV1 {
    idle_stall_started: Arc<Notify>,
    deadline_chunk_started: Arc<Notify>,
}

impl CommunityStreamConformanceExecutorV1 {
    fn new() -> Self {
        Self {
            idle_stall_started: Arc::new(Notify::new()),
            deadline_chunk_started: Arc::new(Notify::new()),
        }
    }

    async fn idle_stall_started(&self) {
        self.idle_stall_started.notified().await;
    }

    async fn deadline_chunk_started(&self) {
        self.deadline_chunk_started.notified().await;
    }

    #[allow(clippy::too_many_lines)] // one canonical case keeps setup, driving and evidence together
    async fn execute_community_stream_case(
        &self,
        fixture: &ProviderStreamFixtureV1,
    ) -> ProviderStreamObservationV1 {
        let resolver_calls = Arc::new(AtomicUsize::new(0));
        let transport_calls = Arc::new(AtomicUsize::new(0));
        let resolver_dropped = Arc::new(AtomicBool::new(false));
        let transport_dropped = Arc::new(AtomicBool::new(false));
        let prepared = match prepare_stream_conformance_input(fixture.input()) {
            Ok(prepared) => prepared,
            Err(error) => {
                return ProviderStreamObservationV1::failure(
                    map_prepare_failure_code(error),
                    ProviderStreamEvidenceV1::new(0, 0, false, false, 0, None),
                );
            }
        };

        let cancellation = CancellationToken::new();
        let cancel_signal = Arc::new(Notify::new());
        let stall_started = match fixture.control() {
            ProviderStreamControlV1::CancelWhileChunkPending => Arc::clone(&cancel_signal),
            ProviderStreamControlV1::AdvanceIdleWhileChunkPending => {
                Arc::clone(&self.idle_stall_started)
            }
            ProviderStreamControlV1::ExpireWhileChunkPending => {
                Arc::clone(&self.deadline_chunk_started)
            }
            ProviderStreamControlV1::Complete => Arc::new(Notify::new()),
        };
        let resolver = ConformanceResolver {
            calls: Arc::clone(&resolver_calls),
            pending: false,
            started: Mutex::new(None),
            dropped: Arc::clone(&resolver_dropped),
        };
        let transport = ConformanceStreamTransport {
            calls: Arc::clone(&transport_calls),
            upstream: *fixture.upstream(),
            stall_started,
            dropped: Arc::clone(&transport_dropped),
        };
        let deadline = matches!(
            fixture.control(),
            ProviderStreamControlV1::ExpireWhileChunkPending
        )
        .then(|| tokio::time::Instant::now() + PROVIDER_STREAM_CONFORMANCE_DEADLINE_OFFSET_V1);
        let evidence = |chunks_pulled, poststream_error_code| {
            ProviderStreamEvidenceV1::new(
                resolver_calls.load(Ordering::SeqCst),
                transport_calls.load(Ordering::SeqCst),
                resolver_dropped.load(Ordering::SeqCst),
                transport_dropped.load(Ordering::SeqCst),
                chunks_pulled,
                poststream_error_code,
            )
        };

        let opened = open_prepared_provider_stream_v1(
            &prepared,
            &resolver,
            &transport,
            deadline,
            &cancellation,
        )
        .await;
        let mut stream = match opened {
            Ok(PreparedProviderStreamResultV1::Opened(stream)) => stream,
            Ok(PreparedProviderStreamResultV1::Rejected(response)) => {
                let head = StreamingResponseHeadV1::try_from_parts(
                    response
                        .status
                        .try_into()
                        .expect("canonical rejection status is valid"),
                    response.headers.get("content-type").cloned(),
                    response.headers.get("retry-after").cloned(),
                )
                .expect("adapter rejection remains in the South contract");
                return ProviderStreamObservationV1::rejected(
                    StreamRejectedV1::new(head, response.body.into_bytes()),
                    evidence(0, None),
                );
            }
            Err(error) => {
                return ProviderStreamObservationV1::failure(
                    map_stable_failure_code(&error),
                    evidence(0, None),
                );
            }
        };

        let head = stream.head().clone();
        let mut chunks = Vec::new();
        let mut poststream_error = None;
        let pull =
            pull_community_stream_until_terminal(&mut stream, &mut chunks, &mut poststream_error);
        if matches!(
            fixture.control(),
            ProviderStreamControlV1::CancelWhileChunkPending
        ) {
            let cancel = async {
                cancel_signal.notified().await;
                cancellation.cancel();
            };
            let ((), ()) = tokio::join!(pull, cancel);
        } else {
            pull.await;
        }

        let observed_evidence = evidence(chunks.len(), poststream_error);
        ProviderStreamObservationV1::opened(head, chunks, observed_evidence)
    }
}

impl AssembledProviderStreamExecutorV1 for CommunityStreamConformanceExecutorV1 {
    fn execute_case<'a>(
        &'a self,
        fixture: &'a ProviderStreamFixtureV1,
    ) -> AssembledStreamExecutionFutureV1<'a> {
        Box::pin(async move { self.execute_community_stream_case(fixture).await })
    }
}

fn prepare_stream_conformance_input(
    input: &ProviderCallInputV1,
) -> Result<PreparedCommunityProviderCallV1, PrepareProviderCallErrorV1> {
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

    prepare_provider_stream_v1(&eligible_streaming_policy(), &provider, &auth, &descriptor)
}

async fn pull_community_stream_until_terminal(
    stream: &mut south_core::StreamingCallV1,
    chunks: &mut Vec<StreamChunkV1>,
    poststream_error: &mut Option<StreamReadErrorV1>,
) {
    loop {
        match stream.next_chunk().await {
            Some(Ok(chunk)) => chunks.push(chunk),
            Some(Err(error)) => {
                *poststream_error = Some(error);
                assert!(
                    stream.next_chunk().await.is_none(),
                    "a terminal stream failure must stick"
                );
                break;
            }
            None => break,
        }
    }
}

struct ConformanceStreamTransport {
    calls: Arc<AtomicUsize>,
    upstream: ProviderStreamUpstreamV1,
    stall_started: Arc<Notify>,
    dropped: Arc<AtomicBool>,
}

impl AsyncStreamingTransport for ConformanceStreamTransport {
    fn open<'a>(&'a self, _request: &'a PreparedHttpRequestV1<'_>) -> StreamingOpenFutureV1<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match self.upstream {
            ProviderStreamUpstreamV1::Stream(raw) => {
                let head = conformance_stream_head(raw.head());
                let source = ConformanceStreamSource {
                    chunks: raw.chunks(),
                    next_index: 0,
                    terminal: raw.terminal(),
                    stall_started: Arc::clone(&self.stall_started),
                    dropped: Arc::clone(&self.dropped),
                };
                Box::pin(async move {
                    OpenedByteStreamV1::try_new(head?, Box::new(source))
                        .map_err(StreamOpenErrorV1::Transport)
                })
            }
            ProviderStreamUpstreamV1::Rejected(raw) => {
                let head = conformance_stream_head(raw.head());
                let body = raw.body().to_vec();
                Box::pin(async move {
                    Err(StreamOpenErrorV1::Rejected(StreamRejectedV1::new(
                        head?, body,
                    )))
                })
            }
            ProviderStreamUpstreamV1::TransportFailure(error) => {
                Box::pin(async move { Err(StreamOpenErrorV1::Transport(error)) })
            }
            ProviderStreamUpstreamV1::NotReached => Box::pin(async {
                Err(StreamOpenErrorV1::Transport(
                    TransportErrorV1::RequestFailed,
                ))
            }),
        }
    }
}

fn conformance_stream_head(
    raw: &ProviderStreamRawHeadV1,
) -> Result<StreamingResponseHeadV1, TransportErrorV1> {
    StreamingResponseHeadV1::try_from_parts(
        raw.status()
            .try_into()
            .map_err(|_| TransportErrorV1::RequestFailed)?,
        raw.content_type().map(str::to_owned),
        raw.retry_after().map(str::to_owned),
    )
}

struct ConformanceStreamSource {
    chunks: &'static [&'static [u8]],
    next_index: usize,
    terminal: ProviderStreamTerminalV1,
    stall_started: Arc<Notify>,
    dropped: Arc<AtomicBool>,
}

impl StreamByteSourceV1 for ConformanceStreamSource {
    fn next_chunk(&mut self) -> StreamChunkFutureV1<'_> {
        Box::pin(async move {
            if let Some(chunk) = self.chunks.get(self.next_index).copied() {
                self.next_index += 1;
                return Some(StreamChunkV1::try_new(chunk.into()));
            }
            match self.terminal {
                ProviderStreamTerminalV1::CleanEof => None,
                ProviderStreamTerminalV1::BreakWithReadFailure => {
                    Some(Err(StreamReadErrorV1::StreamReadFailed))
                }
                ProviderStreamTerminalV1::IdleStall => {
                    self.stall_started.notify_one();
                    tokio::time::sleep(PROVIDER_STREAM_CONFORMANCE_IDLE_TIMEOUT_V1).await;
                    Some(Err(StreamReadErrorV1::StreamIdleTimeout))
                }
                ProviderStreamTerminalV1::PendingForever => {
                    let _drop_probe = PendingDropProbe(Arc::clone(&self.dropped));
                    self.stall_started.notify_one();
                    pending().await
                }
            }
        })
    }
}

struct CommunityConformanceExecutorV1 {
    deadline_transport_started: Arc<Notify>,
}

fn prepare_conformance_fixture(
    fixture: &ProviderCallFixtureV1,
) -> Result<PreparedCommunityProviderCallV1, PrepareProviderCallErrorV1> {
    prepare_conformance_input(fixture.input())
}

fn prepare_conformance_input(
    input: &ProviderCallInputV1,
) -> Result<PreparedCommunityProviderCallV1, PrepareProviderCallErrorV1> {
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

    prepare_provider_call_v1(&eligible_policy(), &provider, &auth, &descriptor)
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
                ProviderCallObservationV1::failure(map_stable_failure_code(&error), evidence)
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

fn map_stable_failure_code(error: &StableProviderCallFailureV1) -> ProviderCallFailureCodeV1 {
    match error {
        StableProviderCallFailureV1::Contract(error) => map_contract_failure_code(*error),
        StableProviderCallFailureV1::Provider(ProviderCallErrorV1::Preparation(error)) => {
            map_preparation_failure_code(*error)
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
        StableProviderCallFailureV1::Provider(ProviderCallErrorV1::Rejected(_)) => {
            ProviderCallFailureCodeV1::RequestFailed
        }
    }
}

fn map_contract_failure_code(error: ContractErrorV1) -> ProviderCallFailureCodeV1 {
    match error {
        ContractErrorV1::InvalidEndpoint => ProviderCallFailureCodeV1::InvalidEndpoint,
        ContractErrorV1::InvalidRelativePath
        | ContractErrorV1::InvalidQueryValue
        | ContractErrorV1::DuplicateQueryParameter
        | ContractErrorV1::EmptyQuery
        | ContractErrorV1::QueryTooLarge
        | ContractErrorV1::InvalidUserAgentValue => ProviderCallFailureCodeV1::InvalidRelativePath,
        ContractErrorV1::InvalidCredentialSlot => ProviderCallFailureCodeV1::InvalidCredentialSlot,
        ContractErrorV1::InvalidJsonBody => ProviderCallFailureCodeV1::InvalidJsonBody,
        ContractErrorV1::RequestBodyTooLarge => ProviderCallFailureCodeV1::RequestBodyTooLarge,
    }
}

#[test]
fn controlled_query_contract_failures_keep_the_frozen_conformance_code() {
    for error in [
        ContractErrorV1::InvalidQueryValue,
        ContractErrorV1::DuplicateQueryParameter,
        ContractErrorV1::EmptyQuery,
        ContractErrorV1::QueryTooLarge,
    ] {
        assert_eq!(
            map_contract_failure_code(error),
            ProviderCallFailureCodeV1::InvalidRelativePath,
        );
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
        // `PreparationErrorV1` is `#[non_exhaustive]` since South 0.7.0: no frozen
        // fixture produces a newer variant, so fold to the context-free fallback and
        // let the runner flag the mismatch.
        _ => ProviderCallFailureCodeV1::RequestFailed,
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

const QUOTA_METADATA_FIELDS_V1: [ProviderQuotaMetadataFieldV1; 9] = [
    ProviderQuotaMetadataFieldV1::XRateLimitLimitTokens,
    ProviderQuotaMetadataFieldV1::XRateLimitRemainingTokens,
    ProviderQuotaMetadataFieldV1::XRateLimitResetTokens,
    ProviderQuotaMetadataFieldV1::AnthropicRateLimitTokensLimit,
    ProviderQuotaMetadataFieldV1::AnthropicRateLimitTokensRemaining,
    ProviderQuotaMetadataFieldV1::AnthropicRateLimitTokensReset,
    ProviderQuotaMetadataFieldV1::AnthropicRateLimitUnifiedLimit,
    ProviderQuotaMetadataFieldV1::AnthropicRateLimitUnifiedRemaining,
    ProviderQuotaMetadataFieldV1::AnthropicRateLimitUnifiedReset,
];

struct CommunityQuotaConformanceExecutorV1;

impl CommunityQuotaConformanceExecutorV1 {
    const fn new() -> Self {
        Self
    }

    async fn execute_community_case(
        &self,
        fixture: &ProviderQuotaMetadataFixtureV1,
    ) -> ProviderQuotaMetadataObservationV1 {
        let prepared = match prepare_conformance_input(fixture.input()) {
            Ok(prepared) => prepared,
            Err(error) => {
                return ProviderQuotaMetadataObservationV1::failure(
                    map_prepare_failure_code(error),
                    ProviderQuotaMetadataEvidenceV1::new(0, 0),
                );
            }
        };
        let resolver = ImmediateResolver {
            calls: AtomicUsize::new(0),
        };
        let transport = QuotaConformanceTransport {
            calls: AtomicUsize::new(0),
            metadata: match fixture.upstream() {
                ProviderQuotaMetadataUpstreamV1::Metadata(raw) => {
                    ProviderQuotaMetadataV1::try_from_iter(
                        QUOTA_METADATA_FIELDS_V1.into_iter().filter_map(|field| {
                            raw.value(field).map(|value| (field, value.to_owned()))
                        }),
                    )
                    .expect("canonical quota metadata is valid")
                }
                // The transport boundary must not be reached for this case; an
                // empty metadata set keeps the transport constructible while the
                // call-count evidence proves it was never asked.
                ProviderQuotaMetadataUpstreamV1::NotReached => ProviderQuotaMetadataV1::default(),
            },
        };
        let result = execute_prepared_provider_call_v1(
            &prepared,
            &resolver,
            &transport,
            tokio::time::Instant::now() + Duration::from_secs(30),
            &CancellationToken::new(),
        )
        .await;
        let evidence = ProviderQuotaMetadataEvidenceV1::new(
            resolver.calls.load(Ordering::SeqCst),
            transport.calls.load(Ordering::SeqCst),
        );

        match result {
            Ok(response) => {
                let projected = ProviderQuotaMetadataV1::try_from_iter(
                    QUOTA_METADATA_FIELDS_V1.into_iter().filter_map(|field| {
                        response
                            .headers
                            .get(field.as_header_name())
                            .map(|value| (field, value.to_owned()))
                    }),
                )
                .expect("host projection remains inside the quota contract");
                ProviderQuotaMetadataObservationV1::response(projected, evidence)
            }
            Err(error) => ProviderQuotaMetadataObservationV1::failure(
                map_stable_failure_code(&error),
                evidence,
            ),
        }
    }
}

impl AssembledProviderQuotaMetadataExecutorV1 for CommunityQuotaConformanceExecutorV1 {
    fn execute_case<'a>(
        &'a self,
        fixture: &'a ProviderQuotaMetadataFixtureV1,
    ) -> AssembledProviderQuotaMetadataExecutionFutureV1<'a> {
        Box::pin(async move { self.execute_community_case(fixture).await })
    }
}

struct QuotaConformanceTransport {
    calls: AtomicUsize,
    metadata: ProviderQuotaMetadataV1,
}

impl AsyncHttpTransport for QuotaConformanceTransport {
    fn execute<'a>(
        &'a self,
        _request: &'a PreparedHttpRequestV1<'_>,
        _remaining_timeout: Duration,
    ) -> TransportFuture<'a> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Box::pin(async move {
            BufferedHttpResponseV1::try_from_parts_with_provider_quota_metadata(
                200_u16.try_into().expect("test status is valid"),
                br"{}".to_vec(),
                None,
                None,
                self.metadata.clone(),
            )
        })
    }
}

#[tokio::test(flavor = "current_thread")]
async fn community_adapter_passes_all_public_quota_metadata_cases() {
    let executor = CommunityQuotaConformanceExecutorV1::new();
    let report = tokio::time::timeout(
        Duration::from_secs(5),
        run_provider_quota_metadata_conformance_v1(&executor),
    )
    .await
    .expect("quota conformance watchdog expired")
    .expect("community adapter must pass the public quota metadata suite");
    assert_eq!(report.passed_case_ids().len(), 3);
}

const QUOTA_LOOPBACK_RESPONSE: &[u8] = concat!(
    "HTTP/1.1 200 OK\r\n",
    "content-type: application/json\r\n",
    "x-ratelimit-limit-tokens: 1000\r\n",
    "x-ratelimit-remaining-tokens: 900\r\n",
    "x-ratelimit-reset-tokens: 10s\r\n",
    "anthropic-ratelimit-tokens-limit: 2000\r\n",
    "anthropic-ratelimit-tokens-remaining: 1500\r\n",
    "anthropic-ratelimit-tokens-reset: 20s\r\n",
    "anthropic-ratelimit-unified-limit: 3000\r\n",
    "anthropic-ratelimit-unified-remaining: 2500\r\n",
    "anthropic-ratelimit-unified-reset: 1970-01-01T00:00:30Z\r\n",
    "x-private-sentinel: must-not-project\r\n",
    "content-length: 2\r\n",
    "connection: close\r\n",
    "\r\n",
    "{}",
)
.as_bytes();

const NO_QUOTA_LOOPBACK_RESPONSE: &[u8] = concat!(
    "HTTP/1.1 200 OK\r\n",
    "content-type: application/json\r\n",
    "content-length: 2\r\n",
    "connection: close\r\n",
    "\r\n",
    "{}",
)
.as_bytes();

struct QuotaFixtureResult {
    windows: Vec<crate::quota_ledger::WindowSnapshot>,
    response_headers: std::collections::BTreeMap<String, String>,
    hit_count: usize,
}

fn start_header_auth_loopback(response: Vec<u8>) -> (String, thread::JoinHandle<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("Header Auth loopback binds");
    let address = listener
        .local_addr()
        .expect("Header Auth loopback has an address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener
            .accept()
            .expect("Header Auth loopback accepts one request");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("Header Auth loopback read timeout is configured");
        let mut received = Vec::new();
        let mut buffer = [0_u8; 4096];
        let expected_len = loop {
            let read = stream
                .read(&mut buffer)
                .expect("Header Auth loopback reads request");
            assert!(read != 0, "Header Auth request ended before its headers");
            received.extend_from_slice(&buffer[..read]);
            if let Some(header_end) = received.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&received[..header_end]);
                let content_length = headers.lines().find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                });
                break header_end + 4 + content_length.unwrap_or(0);
            }
        };
        while received.len() < expected_len {
            let read = stream
                .read(&mut buffer)
                .expect("Header Auth loopback reads request body");
            assert!(read != 0, "Header Auth request body ended early");
            received.extend_from_slice(&buffer[..read]);
        }
        stream
            .write_all(&response)
            .expect("Header Auth loopback writes the immutable response");
        String::from_utf8(received).expect("Header Auth request is valid UTF-8")
    });
    (format!("http://{address}/v1"), server)
}

fn start_quota_loopback(response: Vec<u8>) -> (String, thread::JoinHandle<usize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("quota loopback binds");
    let address = listener
        .local_addr()
        .expect("quota loopback has an address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener
            .accept()
            .expect("quota loopback accepts one request");
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .expect("quota loopback read timeout is configured");
        let mut request = [0_u8; 8 * 1024];
        let bytes_read = stream
            .read(&mut request)
            .expect("quota loopback reads request");
        assert!(bytes_read != 0, "quota loopback received an empty request");
        stream
            .write_all(&response)
            .expect("quota loopback writes the immutable response");
        1
    });
    (format!("http://{address}/v1"), server)
}

fn execute_legacy_quota_fixture() -> QuotaFixtureResult {
    let (base_url, server) = start_quota_loopback(QUOTA_LOOPBACK_RESPONSE.to_vec());
    let response = ureq::get(format!("{base_url}/chat/completions"))
        .call()
        .expect("legacy loopback request succeeds");
    let headers = response
        .headers()
        .iter()
        .filter_map(|(name, value)| {
            value
                .to_str()
                .ok()
                .map(|value| (name.as_str().to_owned(), value.to_owned()))
        })
        .collect();
    let windows = crate::quota_headers::parse_quota_windows(&headers, 0);
    QuotaFixtureResult {
        windows,
        response_headers: headers,
        hit_count: server.join().expect("legacy loopback joins"),
    }
}

async fn execute_south_quota_fixture() -> QuotaFixtureResult {
    let (response, hit_count) =
        execute_south_quota_fixture_with_response(QUOTA_LOOPBACK_RESPONSE.to_vec()).await;
    let windows = crate::quota_headers::parse_quota_windows(&response.headers, 0);
    QuotaFixtureResult {
        windows,
        response_headers: response.headers,
        hit_count,
    }
}

async fn execute_south_quota_fixture_with_response(
    raw_response: Vec<u8>,
) -> (token_station_protocol::HttpResponseParts, usize) {
    let (base_url, server) = start_quota_loopback(raw_response);
    let mut provider = ProviderConfig::new(
        "openai-compatible",
        ProviderEndpoint::try_new(&base_url).expect("South loopback endpoint is valid"),
    );
    provider.auth = Some(SecretRef::new("provider_api_key"));
    let mut descriptor =
        HttpRequestDescriptor::new(HttpMethod::Post, format!("{base_url}/chat/completions"));
    descriptor.headers =
        SafeHeaders::try_new([("content-type", "application/json")]).expect("header is valid");
    descriptor.body = Some(serde_json::json!({"model": "quota-parity"}));
    descriptor.auth = Some(Auth::bearer(SecretRef::new("provider_api_key")));
    let prepared =
        prepare_provider_call_v1(&eligible_policy(), &provider, &auth_config(), &descriptor)
            .expect("South loopback request projects");
    let resolver = ImmediateResolver {
        calls: AtomicUsize::new(0),
    };
    let transport = build_direct_reqwest_transport_v1(
        Duration::from_secs(5),
        Duration::from_secs(2),
        Duration::from_secs(2),
    )
    .expect("South loopback transport builds");
    let response = execute_prepared_provider_call_v1(
        &prepared,
        &resolver,
        &transport,
        tokio::time::Instant::now() + Duration::from_secs(5),
        &CancellationToken::new(),
    )
    .await
    .expect("South loopback request succeeds");
    let hit_count = server.join().expect("South loopback joins");
    (response, hit_count)
}

#[tokio::test(flavor = "current_thread")]
async fn legacy_and_south_produce_identical_quota_windows_from_independent_loopbacks() {
    let legacy = execute_legacy_quota_fixture();
    let south = execute_south_quota_fixture().await;

    assert_eq!(legacy.hit_count, 1);
    assert_eq!(south.hit_count, 1);
    assert_eq!(legacy.windows, south.windows);
    assert_eq!(south.windows.len(), 3);
    assert!(legacy.response_headers.contains_key("x-private-sentinel"));
    assert!(!south.response_headers.contains_key("x-private-sentinel"));
}

fn malformed_quota_loopback_response() -> Vec<u8> {
    let oversized = "x".repeat(MAX_PROVIDER_QUOTA_METADATA_VALUE_BYTES + 1);
    let mut response = format!(
        concat!(
            "HTTP/1.1 200 OK\r\n",
            "content-type: application/json\r\n",
            "x-ratelimit-limit-tokens: 1000\r\n",
            "x-ratelimit-limit-tokens: 1000\r\n",
            "x-ratelimit-remaining-tokens: {oversized}\r\n",
            "x-ratelimit-reset-tokens: "
        ),
        oversized = oversized,
    )
    .into_bytes();
    response.push(0xff);
    response.extend_from_slice(
        concat!(
            "\r\n",
            "anthropic-ratelimit-tokens-limit: 2000\r\n",
            "anthropic-ratelimit-tokens-remaining: 1500\r\n",
            "anthropic-ratelimit-tokens-reset: 20s\r\n",
            "anthropic-ratelimit-unified-limit: 3000\r\n",
            "content-length: 2\r\n",
            "connection: close\r\n",
            "\r\n",
            "{}"
        )
        .as_bytes(),
    );
    response
}

#[tokio::test(flavor = "current_thread")]
async fn south_quota_projection_is_fail_soft_for_absent_partial_and_malformed_fields() {
    let (absent, absent_hits) =
        execute_south_quota_fixture_with_response(NO_QUOTA_LOOPBACK_RESPONSE.to_vec()).await;
    assert_eq!(absent_hits, 1);
    assert_eq!(absent.status, 200);
    assert_eq!(absent.body, "{}");
    assert!(crate::quota_headers::parse_quota_windows(&absent.headers, 0).is_empty());

    let (malformed, malformed_hits) =
        execute_south_quota_fixture_with_response(malformed_quota_loopback_response()).await;
    assert_eq!(malformed_hits, 1);
    assert_eq!(malformed.status, 200);
    assert_eq!(malformed.body, "{}");
    assert!(!malformed.headers.contains_key("x-ratelimit-limit-tokens"));
    assert!(
        !malformed
            .headers
            .contains_key("x-ratelimit-remaining-tokens")
    );
    assert!(!malformed.headers.contains_key("x-ratelimit-reset-tokens"));
    assert_eq!(
        malformed
            .headers
            .get("anthropic-ratelimit-tokens-limit")
            .map(String::as_str),
        Some("2000")
    );
    assert_eq!(
        malformed
            .headers
            .get("anthropic-ratelimit-unified-limit")
            .map(String::as_str),
        Some("3000")
    );
    assert_eq!(
        crate::quota_headers::parse_quota_windows(&malformed.headers, 0).len(),
        1,
        "only the complete Anthropic token family may create a quota window"
    );
}
