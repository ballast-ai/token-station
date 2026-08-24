//! Community-host adapter for South provider-call v1.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    time::Duration,
};

use south_provider_api::AuthArmV1;

use south_contracts::{
    BearerAuthV1, ContractErrorV1, CredentialSlotV1, JsonBodyV1, JsonPostRequestV1,
    PreparationErrorV1, ProviderAuthV1, ProviderEndpointV1, ProviderQuotaMetadataFieldV1,
    RelativePathV1, SafeHeaders, SecretHeaderV1, StreamTransportConfigV1,
};
use south_core::raw::BoundedResolverV1;
use south_core::{
    CredentialResolutionErrorV1, CredentialResolutionFuture, CredentialResolver, ProviderBindingV1,
    SecretValue, StreamingCallV1,
};
use south_transport_reqwest::{
    ReqwestStreamingTransportV1, ReqwestTransportConfigV1, ReqwestTransportV1,
};
use token_station_protocol::{
    Auth, DescriptorError, ErrorCode, ErrorEnvelope, HttpMethod, HttpRequestDescriptor,
    HttpResponseParts, ProviderConfig,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    config::{AuthConfig, EgressMode},
    secrets::SecretStore,
};

const MAX_COMMUNITY_CREDENTIAL_BYTES_V1: usize = 16 * 1024;

const PROVIDER_QUOTA_METADATA_FIELDS_V1: [ProviderQuotaMetadataFieldV1; 9] = [
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

/// Host-owned reasons why a call cannot enter the first South rollout slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IneligibleV1 {
    Egress,
    Streaming,
    Method,
    Auth,
    Body,
    SecretSource,
    Headers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RequestBodyModeV1 {
    Streaming,
    Buffered,
}

/// Static host facts used before any credential lookup or transport call.
#[derive(Clone)]
pub(crate) struct CommunityCallPolicyV1 {
    egress_mode: EgressMode,
    body_mode: RequestBodyModeV1,
    /// The auth shapes the admitted component declares it can carry. Read from
    /// its manifest, which admission has already verified — so eligibility asks
    /// the component what it does rather than matching its dialect against a
    /// list the host has to remember to extend.
    auth_arms: BTreeSet<AuthArmV1>,
}

impl CommunityCallPolicyV1 {
    #[must_use]
    pub(crate) const fn new(
        egress_mode: EgressMode,
        body_mode: RequestBodyModeV1,
        auth_arms: BTreeSet<AuthArmV1>,
    ) -> Self {
        Self {
            egress_mode,
            body_mode,
            auth_arms,
        }
    }
}

impl fmt::Debug for CommunityCallPolicyV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommunityCallPolicyV1")
            .field("egress_mode", &self.egress_mode)
            .field("body_mode", &self.body_mode)
            .field("auth_arms", &self.auth_arms)
            .finish()
    }
}

/// A failure before the assembled South call begins.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PrepareProviderCallErrorV1 {
    Ineligible(IneligibleV1),
    Contract(ContractErrorV1),
    Preparation(PreparationErrorV1),
}

impl From<ContractErrorV1> for PrepareProviderCallErrorV1 {
    fn from(error: ContractErrorV1) -> Self {
        Self::Contract(error)
    }
}

impl From<PreparationErrorV1> for PrepareProviderCallErrorV1 {
    fn from(error: PreparationErrorV1) -> Self {
        Self::Preparation(error)
    }
}

/// A descriptor projected into the bounded South request and binding contracts.
pub(crate) struct PreparedCommunityProviderCallV1 {
    binding: ProviderBindingV1,
    request: JsonPostRequestV1,
}

/// The frozen South failure families that can leave this adapter.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum StableProviderCallFailureV1 {
    Contract(ContractErrorV1),
    Provider(south_core::ProviderCallErrorV1),
}

/// A streaming open either returns a live 2xx pull stream or a bounded non-2xx response.
pub(crate) enum PreparedProviderStreamResultV1 {
    Opened(StreamingCallV1),
    Rejected(HttpResponseParts),
}

/// Host lifecycle context used only to interpret South's context-free `CANCELLED` code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CancellationDispositionV1 {
    ClientDisconnected,
    ServerDrain,
    Deadline,
}

/// A borrowed adapter over the community host's already configured credential store.
pub(crate) struct CommunityCredentialResolverV1<'host> {
    secrets: &'host SecretStore,
    upstream: &'host str,
    declared_slot: &'host str,
}

impl<'host> CommunityCredentialResolverV1<'host> {
    /// Builds the store-backed resolver behind South's size-bounding adapter.
    ///
    /// The 16 KiB cap is a host parameter; the bounding mechanism is the
    /// prelude's `BoundedResolverV1` (South 0.7.0), which replaced this
    /// adapter's hand-rolled length check. An oversized secret maps to the
    /// same opaque resolution failure as before.
    pub(crate) fn try_new(
        secrets: &'host SecretStore,
        upstream: &'host str,
        auth_config: &'host AuthConfig,
    ) -> Result<BoundedResolverV1<Self>, IneligibleV1> {
        if !has_supported_secret_source(auth_config) {
            return Err(IneligibleV1::SecretSource);
        }
        Ok(BoundedResolverV1::new(
            Self {
                secrets,
                upstream,
                declared_slot: &auth_config.slot,
            },
            MAX_COMMUNITY_CREDENTIAL_BYTES_V1,
        ))
    }
}

impl fmt::Debug for CommunityCredentialResolverV1<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommunityCredentialResolverV1")
            .finish_non_exhaustive()
    }
}

impl CredentialResolver for CommunityCredentialResolverV1<'_> {
    fn resolve<'a>(&'a self, slot: &'a CredentialSlotV1) -> CredentialResolutionFuture<'a> {
        Box::pin(async move {
            if slot.as_str() != self.declared_slot {
                return Err(CredentialResolutionErrorV1);
            }
            let secret = self
                .secrets
                .resolve(self.upstream, self.declared_slot)
                .map_err(|_| CredentialResolutionErrorV1)?;
            Ok(SecretValue::new(secret))
        })
    }
}

impl PreparedCommunityProviderCallV1 {
    #[cfg(test)]
    pub(crate) fn relative_path(&self) -> &str {
        self.request.relative_path().as_str()
    }

    #[cfg(test)]
    pub(crate) fn credential_slot(&self) -> &str {
        self.request.auth().credential_slot().as_str()
    }

    #[cfg(test)]
    pub(crate) fn header_count(&self) -> usize {
        self.request.headers().len()
    }

    #[cfg(test)]
    pub(crate) fn body(&self) -> &str {
        self.request.body().as_str()
    }
}

impl fmt::Debug for PreparedCommunityProviderCallV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PreparedCommunityProviderCallV1")
            .field("request", &self.request)
            .finish_non_exhaustive()
    }
}

/// Applies every host and contract check before any secret or transport capability is available.
pub(crate) fn prepare_provider_call_v1(
    policy: &CommunityCallPolicyV1,
    provider: &ProviderConfig,
    auth_config: &AuthConfig,
    descriptor: &HttpRequestDescriptor,
) -> Result<PreparedCommunityProviderCallV1, PrepareProviderCallErrorV1> {
    prepare_provider_call_for_mode_v1(
        policy,
        RequestBodyModeV1::Buffered,
        provider,
        auth_config,
        descriptor,
    )
}

/// Projects an explicitly streaming-eligible descriptor into the same bounded request contract.
pub(crate) fn prepare_provider_stream_v1(
    policy: &CommunityCallPolicyV1,
    provider: &ProviderConfig,
    auth_config: &AuthConfig,
    descriptor: &HttpRequestDescriptor,
) -> Result<PreparedCommunityProviderCallV1, PrepareProviderCallErrorV1> {
    prepare_provider_call_for_mode_v1(
        policy,
        RequestBodyModeV1::Streaming,
        provider,
        auth_config,
        descriptor,
    )
}

fn prepare_provider_call_for_mode_v1(
    policy: &CommunityCallPolicyV1,
    expected_body_mode: RequestBodyModeV1,
    provider: &ProviderConfig,
    auth_config: &AuthConfig,
    descriptor: &HttpRequestDescriptor,
) -> Result<PreparedCommunityProviderCallV1, PrepareProviderCallErrorV1> {
    check_static_eligibility(
        policy,
        expected_body_mode,
        provider,
        auth_config,
        descriptor,
    )?;

    let endpoint = ProviderEndpointV1::parse(&provider.base_url.as_str())?;
    let relative_path = project_relative_path(&endpoint, &descriptor.url)?;
    let bound_slot = provider
        .auth
        .as_ref()
        .ok_or(PrepareProviderCallErrorV1::Ineligible(IneligibleV1::Auth))?;
    let bound_slot = CredentialSlotV1::parse(bound_slot.as_str())?;
    let requested_auth = match descriptor.auth.as_ref() {
        Some(Auth::Bearer { secret }) => {
            ProviderAuthV1::from(BearerAuthV1::new(CredentialSlotV1::parse(secret.as_str())?))
        }
        Some(Auth::Header { name, secret })
            if policy.auth_arms.contains(&AuthArmV1::HeaderSecret)
                && sanctioned_secret_header(name).is_some() =>
        {
            let header = sanctioned_secret_header(name)
                .ok_or(PrepareProviderCallErrorV1::Ineligible(IneligibleV1::Auth))?;
            ProviderAuthV1::HeaderSecret {
                header,
                slot: BearerAuthV1::new(CredentialSlotV1::parse(secret.as_str())?),
            }
        }
        _ => return Err(PrepareProviderCallErrorV1::Ineligible(IneligibleV1::Auth)),
    };

    provider
        .authorize(descriptor)
        .map_err(|error| map_legacy_authorization(&error))?;

    let headers = SafeHeaders::try_from_iter(
        descriptor
            .headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
    )
    .map_err(|_| PrepareProviderCallErrorV1::Ineligible(IneligibleV1::Headers))?;
    let body = descriptor
        .body
        .as_ref()
        .ok_or(PrepareProviderCallErrorV1::Ineligible(IneligibleV1::Body))?;
    let encoded = serde_json::to_string(body).map_err(|_| ContractErrorV1::InvalidJsonBody)?;
    let body = JsonBodyV1::parse(&encoded)?;

    Ok(PreparedCommunityProviderCallV1 {
        binding: ProviderBindingV1::new(endpoint, bound_slot),
        request: JsonPostRequestV1::new(relative_path, headers, body, requested_auth),
    })
}

/// Runs the real South orchestrator with caller-injected capabilities and projects its response.
pub(crate) async fn execute_prepared_provider_call_v1<R, T>(
    prepared: &PreparedCommunityProviderCallV1,
    resolver: &R,
    transport: &T,
    deadline: Instant,
    cancellation: &CancellationToken,
) -> Result<HttpResponseParts, StableProviderCallFailureV1>
where
    R: south_core::CredentialResolver + ?Sized,
    T: south_core::AsyncHttpTransport + ?Sized,
{
    let response = south_core::execute_provider_call_v1(
        &prepared.binding,
        &prepared.request,
        resolver,
        transport,
        deadline,
        cancellation,
    )
    .await
    .map_err(StableProviderCallFailureV1::Provider)?;

    let mut headers = BTreeMap::new();
    if let Some(content_type) = response.content_type() {
        headers.insert("content-type".to_owned(), content_type.to_owned());
    }
    if let Some(retry_after) = response.retry_after() {
        headers.insert("retry-after".to_owned(), retry_after.to_owned());
    }
    for field in PROVIDER_QUOTA_METADATA_FIELDS_V1 {
        if let Some(value) = response.provider_quota_metadata().value(field) {
            headers.insert(field.as_header_name().to_owned(), value.to_owned());
        }
    }
    Ok(HttpResponseParts {
        status: response.status().as_u16(),
        headers,
        body: response.body().to_owned(),
        extensions: BTreeMap::new(),
    })
}

/// Opens the real South stream and projects a bounded non-2xx rejection for host classification.
pub(crate) async fn open_prepared_provider_stream_v1<R, T>(
    prepared: &PreparedCommunityProviderCallV1,
    resolver: &R,
    transport: &T,
    deadline: Option<Instant>,
    cancellation: &CancellationToken,
) -> Result<PreparedProviderStreamResultV1, StableProviderCallFailureV1>
where
    R: south_core::CredentialResolver + ?Sized,
    T: south_core::AsyncStreamingTransport + ?Sized,
{
    match south_core::open_streaming_provider_call_v1(
        &prepared.binding,
        &prepared.request,
        resolver,
        transport,
        deadline,
        cancellation,
    )
    .await
    {
        Ok(stream) => Ok(PreparedProviderStreamResultV1::Opened(stream)),
        Err(south_core::ProviderCallErrorV1::Rejected(rejected)) => {
            let body = String::from_utf8_lossy(rejected.body()).into_owned();
            Ok(PreparedProviderStreamResultV1::Rejected(
                HttpResponseParts {
                    status: rejected.head().status().as_u16(),
                    headers: project_stream_head_headers(rejected.head()),
                    body,
                    extensions: BTreeMap::new(),
                },
            ))
        }
        Err(error) => Err(StableProviderCallFailureV1::Provider(error)),
    }
}

fn project_stream_head_headers(
    head: &south_contracts::StreamingResponseHeadV1,
) -> BTreeMap<String, String> {
    let mut headers = BTreeMap::new();
    if let Some(content_type) = head.content_type() {
        headers.insert("content-type".to_owned(), content_type.to_owned());
    }
    if let Some(retry_after) = head.retry_after() {
        headers.insert("retry-after".to_owned(), retry_after.to_owned());
    }
    for field in PROVIDER_QUOTA_METADATA_FIELDS_V1 {
        if let Some(value) = head.provider_quota_metadata().value(field) {
            headers.insert(field.as_header_name().to_owned(), value.to_owned());
        }
    }
    headers
}

/// Projects the content-free head fields needed by the host before body streaming begins.
pub(crate) fn project_open_stream_head_v1(
    stream: &StreamingCallV1,
) -> (u16, BTreeMap<String, String>) {
    (
        stream.head().status().as_u16(),
        project_stream_head_headers(stream.head()),
    )
}

/// Builds the only real transport shape permitted by the first community rollout slice.
pub(crate) fn build_direct_reqwest_transport_v1(
    total_timeout: Duration,
    connect_timeout: Duration,
    read_timeout: Duration,
) -> Result<ReqwestTransportV1, StableProviderCallFailureV1> {
    let config = ReqwestTransportConfigV1::try_new(total_timeout, connect_timeout, read_timeout)
        .map_err(|error| {
            StableProviderCallFailureV1::Provider(south_core::ProviderCallErrorV1::Transport(error))
        })?;
    ReqwestTransportV1::new(config).map_err(|error| {
        StableProviderCallFailureV1::Provider(south_core::ProviderCallErrorV1::Transport(error))
    })
}

/// Builds the dedicated streaming transport with mandatory connect and idle guards.
pub(crate) fn build_direct_reqwest_streaming_transport_v1(
    total_timeout: Option<Duration>,
    connect_timeout: Duration,
    idle_timeout: Duration,
) -> Result<ReqwestStreamingTransportV1, StableProviderCallFailureV1> {
    let config = StreamTransportConfigV1::try_new(total_timeout, connect_timeout, idle_timeout)
        .map_err(|error| {
            StableProviderCallFailureV1::Provider(south_core::ProviderCallErrorV1::Transport(error))
        })?;
    ReqwestStreamingTransportV1::new(config).map_err(|error| {
        StableProviderCallFailureV1::Provider(south_core::ProviderCallErrorV1::Transport(error))
    })
}

/// Maps every frozen South v1 code to the community host's existing error catalog.
pub(crate) fn map_failure_v1(
    failure: StableProviderCallFailureV1,
    cancellation: CancellationDispositionV1,
) -> ErrorEnvelope {
    let (code, status, message) = match failure {
        StableProviderCallFailureV1::Contract(error) => match error {
            ContractErrorV1::RequestBodyTooLarge => (
                ErrorCode::InvalidRequest,
                413,
                "provider request body is too large",
            ),
            ContractErrorV1::InvalidEndpoint
            | ContractErrorV1::InvalidRelativePath
            | ContractErrorV1::InvalidCredentialSlot
            | ContractErrorV1::InvalidJsonBody
            | ContractErrorV1::InvalidQueryValue
            | ContractErrorV1::DuplicateQueryParameter
            | ContractErrorV1::EmptyQuery
            | ContractErrorV1::QueryTooLarge
            | ContractErrorV1::InvalidUserAgentValue => (
                ErrorCode::Internal,
                500,
                "provider request contract rejected",
            ),
        },
        StableProviderCallFailureV1::Provider(error) => match error {
            south_core::ProviderCallErrorV1::Preparation(error) => match error {
                PreparationErrorV1::UrlOutsideBinding
                | PreparationErrorV1::CredentialBindingMismatch => {
                    (ErrorCode::Internal, 500, "provider binding rejected")
                }
                PreparationErrorV1::CredentialResolutionFailed => (
                    ErrorCode::Auth,
                    401,
                    "provider credential resolution failed",
                ),
                PreparationErrorV1::Cancelled => match cancellation {
                    CancellationDispositionV1::ClientDisconnected => {
                        (ErrorCode::Internal, 499, "request cancelled")
                    }
                    CancellationDispositionV1::ServerDrain => (
                        ErrorCode::UpstreamUnavailable,
                        503,
                        "Token Station is restarting. Retry this request.",
                    ),
                    CancellationDispositionV1::Deadline => {
                        (ErrorCode::Timeout, 504, "request deadline exceeded")
                    }
                },
                PreparationErrorV1::DeadlineExceeded => {
                    (ErrorCode::Timeout, 504, "request deadline exceeded")
                }
                // `PreparationErrorV1` is `#[non_exhaustive]` since South 0.7.0
                // (host-prelude D2/D4; includes `UNSUPPORTED_AUTH_SHAPE`). The
                // community adapter only declares the two frozen auth arms, so a
                // newer variant is structurally unreachable here; fail closed as
                // an internal preparation rejection rather than panicking.
                _ => (
                    ErrorCode::Internal,
                    500,
                    "provider request preparation rejected",
                ),
            },
            south_core::ProviderCallErrorV1::Transport(error) => match error {
                south_contracts::TransportErrorV1::ClientBuildFailed => (
                    ErrorCode::Internal,
                    500,
                    "provider transport is unavailable",
                ),
                south_contracts::TransportErrorV1::TransportTimeout => {
                    (ErrorCode::Timeout, 504, "provider transport timed out")
                }
                south_contracts::TransportErrorV1::ConnectFailed
                | south_contracts::TransportErrorV1::RequestFailed
                | south_contracts::TransportErrorV1::RedirectDenied => (
                    ErrorCode::UpstreamUnavailable,
                    502,
                    "provider transport failed",
                ),
                south_contracts::TransportErrorV1::ResponseReadFailed => (
                    ErrorCode::TransportTruncated,
                    502,
                    "provider response was truncated",
                ),
                south_contracts::TransportErrorV1::ResponseBodyTooLarge
                | south_contracts::TransportErrorV1::ResponseBodyNotUtf8
                | south_contracts::TransportErrorV1::ResponseMetadataInvalid => (
                    ErrorCode::ProviderProtocolError,
                    502,
                    "provider response contract rejected",
                ),
            },
            south_core::ProviderCallErrorV1::Rejected(_) => (
                ErrorCode::ProviderProtocolError,
                502,
                "buffered provider call returned a streaming rejection",
            ),
        },
    };
    ErrorEnvelope::new(code, status, message)
}

/// Maps a terminal South stream pull code without inspecting response bytes.
pub(crate) fn map_stream_read_failure_v1(
    failure: south_contracts::StreamReadErrorV1,
    cancellation: CancellationDispositionV1,
) -> ErrorEnvelope {
    let (code, status, message) = match failure {
        south_contracts::StreamReadErrorV1::StreamReadFailed => (
            ErrorCode::TransportTruncated,
            502,
            "provider response was truncated",
        ),
        south_contracts::StreamReadErrorV1::StreamIdleTimeout => {
            (ErrorCode::Timeout, 504, "provider stream became idle")
        }
        south_contracts::StreamReadErrorV1::StreamDeadlineExceeded => {
            (ErrorCode::Timeout, 504, "request deadline exceeded")
        }
        south_contracts::StreamReadErrorV1::StreamCancelled => match cancellation {
            CancellationDispositionV1::ClientDisconnected => {
                (ErrorCode::Internal, 499, "request cancelled")
            }
            CancellationDispositionV1::ServerDrain => (
                ErrorCode::UpstreamUnavailable,
                503,
                "Token Station is restarting. Retry this request.",
            ),
            CancellationDispositionV1::Deadline => {
                (ErrorCode::Timeout, 504, "request deadline exceeded")
            }
        },
        south_contracts::StreamReadErrorV1::ChunkNotDeliverable => (
            ErrorCode::ProviderProtocolError,
            502,
            "provider stream chunk was rejected",
        ),
    };
    ErrorEnvelope::new(code, status, message)
}

fn check_static_eligibility(
    policy: &CommunityCallPolicyV1,
    expected_body_mode: RequestBodyModeV1,
    provider: &ProviderConfig,
    auth_config: &AuthConfig,
    descriptor: &HttpRequestDescriptor,
) -> Result<(), PrepareProviderCallErrorV1> {
    let ineligible = |reason| Err(PrepareProviderCallErrorV1::Ineligible(reason));
    if policy.egress_mode != EgressMode::Direct {
        return ineligible(IneligibleV1::Egress);
    }
    if policy.body_mode != expected_body_mode {
        return ineligible(IneligibleV1::Streaming);
    }
    if descriptor.method != HttpMethod::Post {
        return ineligible(IneligibleV1::Method);
    }
    if descriptor.body.is_none() {
        return ineligible(IneligibleV1::Body);
    }
    // The descriptor says what this request needs; the manifest says what the
    // component carries. Anything else — the dialect's name in particular — is
    // not the transport's business once admission has run.
    let supported_auth = match descriptor.auth.as_ref() {
        Some(Auth::Bearer { .. }) => policy.auth_arms.contains(&AuthArmV1::Bearer),
        Some(Auth::Header { name, .. }) => {
            policy.auth_arms.contains(&AuthArmV1::HeaderSecret)
                && sanctioned_secret_header(name).is_some()
        }
        _ => false,
    };
    if !supported_auth || provider.auth.is_none() {
        return ineligible(IneligibleV1::Auth);
    }
    if !has_supported_secret_source(auth_config) {
        return ineligible(IneligibleV1::SecretSource);
    }
    if provider
        .auth
        .as_ref()
        .map(token_station_protocol::SecretRef::as_str)
        != Some(auth_config.slot.as_str())
    {
        return Err(PreparationErrorV1::CredentialBindingMismatch.into());
    }
    Ok(())
}

fn sanctioned_secret_header(name: &str) -> Option<SecretHeaderV1> {
    SecretHeaderV1::ALL
        .into_iter()
        .find(|header| header.header_name().eq_ignore_ascii_case(name))
}

fn has_supported_secret_source(auth_config: &AuthConfig) -> bool {
    let source_count = usize::from(auth_config.store)
        + usize::from(auth_config.env.is_some())
        + usize::from(auth_config.file.is_some());
    auth_config.file.is_none() && source_count == 1
}

fn project_relative_path(
    endpoint: &ProviderEndpointV1,
    descriptor_url: &str,
) -> Result<RelativePathV1, PrepareProviderCallErrorV1> {
    let trusted = Url::parse(endpoint.as_str())
        .map_err(|_| PrepareProviderCallErrorV1::Contract(ContractErrorV1::InvalidEndpoint))?;
    let target = Url::parse(descriptor_url).map_err(|_| {
        PrepareProviderCallErrorV1::Preparation(PreparationErrorV1::UrlOutsideBinding)
    })?;
    if !target.username().is_empty()
        || target.password().is_some()
        || target.query().is_some()
        || target.fragment().is_some()
        || target.scheme() != trusted.scheme()
        || target.host_str() != trusted.host_str()
        || target.port_or_known_default() != trusted.port_or_known_default()
    {
        return Err(PreparationErrorV1::UrlOutsideBinding.into());
    }

    let raw_path = raw_absolute_path(descriptor_url).ok_or(
        PrepareProviderCallErrorV1::Preparation(PreparationErrorV1::UrlOutsideBinding),
    )?;
    let relative =
        raw_path
            .strip_prefix(trusted.path())
            .ok_or(PrepareProviderCallErrorV1::Preparation(
                PreparationErrorV1::UrlOutsideBinding,
            ))?;
    let relative = RelativePathV1::parse(relative)?;
    let resolved = relative.resolve_against(endpoint)?;
    if resolved != target {
        return Err(PreparationErrorV1::UrlOutsideBinding.into());
    }
    Ok(relative)
}

fn raw_absolute_path(url: &str) -> Option<&str> {
    let scheme_end = url.find("://")?;
    let authority_and_path = url.get(scheme_end + 3..)?;
    let path_start = authority_and_path.find('/');
    match path_start {
        Some(index) => authority_and_path.get(index..),
        None => Some("/"),
    }
}

fn map_legacy_authorization(error: &DescriptorError) -> PrepareProviderCallErrorV1 {
    match error {
        DescriptorError::UrlOutsideEndpoint { .. } => {
            PrepareProviderCallErrorV1::Preparation(PreparationErrorV1::UrlOutsideBinding)
        }
        DescriptorError::UndeclaredCredential { .. }
        | DescriptorError::UnexpectedCredential { .. }
        | DescriptorError::MissingCredential => {
            PrepareProviderCallErrorV1::Preparation(PreparationErrorV1::CredentialBindingMismatch)
        }
    }
}
