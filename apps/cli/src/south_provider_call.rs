//! Community-host adapter for South provider-call v1.

use std::{collections::BTreeMap, fmt, time::Duration};

use south_contracts::{
    BearerAuthV1, ContractErrorV1, CredentialSlotV1, JsonBodyV1, JsonPostRequestV1,
    PreparationErrorV1, ProviderEndpointV1, RelativePathV1, SafeHeaders,
};
use south_core::{
    CredentialResolutionErrorV1, CredentialResolutionFuture, CredentialResolver, ProviderBindingV1,
    SecretValue,
};
use south_transport_reqwest::{ReqwestTransportConfigV1, ReqwestTransportV1};
use token_station_protocol::{
    Auth, DescriptorError, ErrorCode, ErrorEnvelope, HttpMethod, HttpRequestDescriptor,
    HttpResponseParts, ProviderConfig,
};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;
use url::Url;

use crate::{
    config::{ApiDialect, AuthConfig, EgressMode},
    secrets::SecretStore,
};

const MAX_COMMUNITY_CREDENTIAL_BYTES_V1: usize = 16 * 1024;

/// Host-owned reasons why a call cannot enter the first South rollout slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum IneligibleV1 {
    RolloutDisabled,
    ProviderDialect,
    ProviderPackageUnapproved,
    ApiDialect,
    Egress,
    Streaming,
    Method,
    Auth,
    Body,
    SecretSource,
    ResponseMetadata,
    Headers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RolloutEligibilityV1 {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "production rollout wiring begins after diagnostics"
        )
    )]
    Disabled,
    Enabled,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProviderPackageEligibilityV1 {
    Unapproved,
    Approved,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum RequestBodyModeV1 {
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "streaming remains outside South provider-call v1")
    )]
    Streaming,
    Buffered,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ResponseMetadataEligibilityV1 {
    Incompatible,
    Compatible,
}

/// Static host facts used before any credential lookup or transport call.
#[derive(Clone, Copy)]
pub(crate) struct CommunityCallPolicyV1 {
    rollout: RolloutEligibilityV1,
    provider_package: ProviderPackageEligibilityV1,
    api_dialect: ApiDialect,
    egress_mode: EgressMode,
    body_mode: RequestBodyModeV1,
    response_metadata: ResponseMetadataEligibilityV1,
}

impl CommunityCallPolicyV1 {
    #[must_use]
    pub(crate) const fn new(
        rollout: RolloutEligibilityV1,
        provider_package: ProviderPackageEligibilityV1,
        api_dialect: ApiDialect,
        egress_mode: EgressMode,
        body_mode: RequestBodyModeV1,
        response_metadata: ResponseMetadataEligibilityV1,
    ) -> Self {
        Self {
            rollout,
            provider_package,
            api_dialect,
            egress_mode,
            body_mode,
            response_metadata,
        }
    }
}

impl fmt::Debug for CommunityCallPolicyV1 {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CommunityCallPolicyV1")
            .field("rollout", &self.rollout)
            .field("provider_package", &self.provider_package)
            .field("api_dialect", &self.api_dialect)
            .field("egress_mode", &self.egress_mode)
            .field("body_mode", &self.body_mode)
            .field("response_metadata", &self.response_metadata)
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StableProviderCallFailureV1 {
    Contract(ContractErrorV1),
    Provider(south_core::ProviderCallErrorV1),
}

/// Host lifecycle context used only to interpret South's context-free `CANCELLED` code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CancellationDispositionV1 {
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "client cancellation is wired with production traffic"
        )
    )]
    ClientDisconnected,
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "server drain is wired with production traffic")
    )]
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
    pub(crate) fn try_new(
        secrets: &'host SecretStore,
        upstream: &'host str,
        auth_config: &'host AuthConfig,
    ) -> Result<Self, IneligibleV1> {
        if !has_supported_secret_source(auth_config) {
            return Err(IneligibleV1::SecretSource);
        }
        Ok(Self {
            secrets,
            upstream,
            declared_slot: &auth_config.slot,
        })
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
            if secret.len() > MAX_COMMUNITY_CREDENTIAL_BYTES_V1 {
                return Err(CredentialResolutionErrorV1);
            }
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
    policy: CommunityCallPolicyV1,
    provider: &ProviderConfig,
    auth_config: &AuthConfig,
    descriptor: &HttpRequestDescriptor,
) -> Result<PreparedCommunityProviderCallV1, PrepareProviderCallErrorV1> {
    check_static_eligibility(policy, provider, auth_config, descriptor)?;

    let endpoint = ProviderEndpointV1::parse(&provider.base_url.as_str())?;
    let relative_path = project_relative_path(&endpoint, &descriptor.url)?;
    let bound_slot = provider
        .auth
        .as_ref()
        .ok_or(PrepareProviderCallErrorV1::Ineligible(IneligibleV1::Auth))?;
    let bound_slot = CredentialSlotV1::parse(bound_slot.as_str())?;
    let requested_slot = match descriptor.auth.as_ref() {
        Some(Auth::Bearer { secret }) => CredentialSlotV1::parse(secret.as_str())?,
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
        request: JsonPostRequestV1::new(
            relative_path,
            headers,
            body,
            BearerAuthV1::new(requested_slot),
        ),
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
    Ok(HttpResponseParts {
        status: response.status().as_u16(),
        headers,
        body: response.body().to_owned(),
        extensions: BTreeMap::new(),
    })
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
            | ContractErrorV1::InvalidJsonBody => (
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
                        "Token Station is restarting; retry this request",
                    ),
                    CancellationDispositionV1::Deadline => {
                        (ErrorCode::Timeout, 504, "request deadline exceeded")
                    }
                },
                PreparationErrorV1::DeadlineExceeded => {
                    (ErrorCode::Timeout, 504, "request deadline exceeded")
                }
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
        },
    };
    ErrorEnvelope::new(code, status, message)
}

fn check_static_eligibility(
    policy: CommunityCallPolicyV1,
    provider: &ProviderConfig,
    auth_config: &AuthConfig,
    descriptor: &HttpRequestDescriptor,
) -> Result<(), PrepareProviderCallErrorV1> {
    let ineligible = |reason| Err(PrepareProviderCallErrorV1::Ineligible(reason));
    if policy.rollout != RolloutEligibilityV1::Enabled {
        return ineligible(IneligibleV1::RolloutDisabled);
    }
    if provider.provider != "openai-compatible" {
        return ineligible(IneligibleV1::ProviderDialect);
    }
    if policy.provider_package != ProviderPackageEligibilityV1::Approved {
        return ineligible(IneligibleV1::ProviderPackageUnapproved);
    }
    if policy.api_dialect != ApiDialect::Translated {
        return ineligible(IneligibleV1::ApiDialect);
    }
    if policy.egress_mode != EgressMode::Direct {
        return ineligible(IneligibleV1::Egress);
    }
    if policy.body_mode != RequestBodyModeV1::Buffered {
        return ineligible(IneligibleV1::Streaming);
    }
    if policy.response_metadata != ResponseMetadataEligibilityV1::Compatible {
        return ineligible(IneligibleV1::ResponseMetadata);
    }
    if descriptor.method != HttpMethod::Post {
        return ineligible(IneligibleV1::Method);
    }
    if descriptor.body.is_none() {
        return ineligible(IneligibleV1::Body);
    }
    if !matches!(descriptor.auth, Some(Auth::Bearer { .. })) || provider.auth.is_none() {
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
