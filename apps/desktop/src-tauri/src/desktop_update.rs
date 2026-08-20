use std::future::Future;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use serde::Serialize;

pub const RELEASES_URL: &str = "https://github.com/ballast-ai/token-station/releases";
pub const STABLE_LATEST_JSON_URL: &str =
    "https://github.com/ballast-ai/token-station/releases/latest/download/latest.json";
const CONFIGURED_LATEST_JSON_URL: Option<&str> = option_env!("TOKEN_STATION_UPDATER_ENDPOINT");
pub const PROGRESS_EVENT: &str = "desktop-update-progress";
pub const WINDOWS_FIRST_RELEASE_UNSUPPORTED_MESSAGE: &str =
    "Windows 首版暂不支持应用内更新；请从正式发布页手动下载安装。";
pub const MACOS_ONLY_FIRST_RELEASE_UNSUPPORTED_MESSAGE: &str =
    "首版应用内更新仅支持 macOS；请从正式发布页手动下载安装。";

/// The private key never reaches the build. Production builds inject only the
/// matching public key; source/local builds deliberately compile without one.
pub const OFFICIAL_PUBLIC_KEY: &str = match option_env!("TOKEN_STATION_UPDATER_PUBKEY") {
    Some(public_key) => public_key,
    None => "",
};

pub fn update_manifest_endpoint(configured: Option<&str>) -> Result<&str, String> {
    let endpoint = configured
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(STABLE_LATEST_JSON_URL);
    if !endpoint.starts_with("https://") {
        return Err(
            "update_manifest_endpoint_invalid: updater manifest endpoint must use HTTPS".to_owned(),
        );
    }
    Ok(endpoint)
}

pub fn official_update_manifest_endpoint() -> Result<&'static str, String> {
    update_manifest_endpoint(CONFIGURED_LATEST_JSON_URL)
}

#[derive(Clone, Default)]
pub struct DesktopUpdateOperation {
    active: Arc<AtomicBool>,
}

impl DesktopUpdateOperation {
    pub fn try_begin(&self) -> Result<DesktopUpdateLease, String> {
        self.active
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| "update_in_progress: 已有更新任务正在进行".to_owned())?;
        Ok(DesktopUpdateLease {
            active: Arc::clone(&self.active),
        })
    }
}

pub struct DesktopUpdateLease {
    active: Arc<AtomicBool>,
}

impl Drop for DesktopUpdateLease {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopUpdatePrepareFailure<T> {
    message: String,
    recovery: T,
}

impl<T> DesktopUpdatePrepareFailure<T> {
    pub fn new(message: impl Into<String>, recovery: T) -> Self {
        Self {
            message: message.into(),
            recovery,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DesktopUpdateProgress {
    pub downloaded: u64,
    pub total: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DesktopUpdateStatus {
    UpToDate,
    UpdateAvailable,
    Unsupported,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopUpdateCandidate {
    pub version: String,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct DesktopUpdateView {
    pub status: DesktopUpdateStatus,
    pub current_version: String,
    pub version: Option<String>,
    pub notes: Option<String>,
    pub pub_date: Option<String>,
    pub release_url: String,
    pub message: Option<String>,
}

impl DesktopUpdateView {
    pub fn unsupported(current_version: &str, message: impl Into<String>) -> Self {
        Self {
            status: DesktopUpdateStatus::Unsupported,
            current_version: current_version.to_owned(),
            version: None,
            notes: None,
            pub_date: None,
            release_url: RELEASES_URL.to_owned(),
            message: Some(message.into()),
        }
    }
}

/// Checks the public update seam while keeping network access injectable in tests.
/// A build without the official public key refuses before contacting the update source.
pub async fn check_with<F, Fut>(
    current_version: &str,
    public_key: &str,
    check: F,
) -> DesktopUpdateView
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = Result<Option<DesktopUpdateCandidate>, String>>,
{
    if public_key.trim().is_empty() {
        return DesktopUpdateView::unsupported(
            current_version,
            "当前构建没有内置官方更新公钥，不能在 App 内安装更新；请从正式发布页手动下载安装。",
        );
    }

    match check().await {
        Ok(Some(candidate)) => DesktopUpdateView {
            status: DesktopUpdateStatus::UpdateAvailable,
            current_version: current_version.to_owned(),
            release_url: format!("{RELEASES_URL}/tag/v{}", candidate.version),
            version: Some(candidate.version),
            notes: candidate.notes,
            pub_date: candidate.pub_date,
            message: None,
        },
        Ok(None) => DesktopUpdateView {
            status: DesktopUpdateStatus::UpToDate,
            current_version: current_version.to_owned(),
            version: None,
            notes: None,
            pub_date: None,
            release_url: RELEASES_URL.to_owned(),
            message: None,
        },
        Err(message) => DesktopUpdateView {
            status: DesktopUpdateStatus::Unavailable,
            current_version: current_version.to_owned(),
            version: None,
            notes: None,
            pub_date: None,
            release_url: RELEASES_URL.to_owned(),
            message: Some(message),
        },
    }
}

/// Runs the security-sensitive install order around injectable system boundaries.
/// `download` must return only after the updater signature has verified; therefore
/// gateway preparation and installation are unreachable on a bad download/signature.
pub async fn install_with<
    P,
    Prepared,
    Check,
    CheckFuture,
    Download,
    DownloadFuture,
    Prepare,
    PrepareFuture,
    Install,
    Recover,
    RecoverFuture,
>(
    public_key: &str,
    expected_version: &str,
    check: Check,
    download: Download,
    prepare: Prepare,
    install: Install,
    recover: Recover,
) -> Result<bool, String>
where
    Check: FnOnce() -> CheckFuture,
    CheckFuture: Future<Output = Result<Option<(String, P)>, String>>,
    Download: FnOnce(P) -> DownloadFuture,
    DownloadFuture: Future<Output = Result<(P, Vec<u8>), String>>,
    Prepare: FnOnce() -> PrepareFuture,
    PrepareFuture: Future<Output = Result<Prepared, DesktopUpdatePrepareFailure<Prepared>>>,
    Install: FnOnce(P, Vec<u8>, &Prepared) -> Result<(), String>,
    Recover: FnOnce(Prepared) -> RecoverFuture,
    RecoverFuture: Future<Output = Result<(), String>>,
{
    if public_key.trim().is_empty() {
        return Err("当前构建没有内置官方更新公钥，不能在 App 内安装更新。".to_owned());
    }
    let expected_version = expected_version.trim();
    if expected_version.is_empty() {
        return Err("update_expected_version_missing: 缺少已确认的更新版本，请重新检查".to_owned());
    }

    let Some((available_version, package)) = check().await? else {
        return Ok(false);
    };
    if available_version != expected_version {
        return Err(format!(
            "update_version_changed: 已确认更新到 {expected_version}，但当前可用版本已变为 {available_version}；请重新检查并确认"
        ));
    }
    let (package, bytes) = download(package).await?;
    let prepared = match prepare().await {
        Ok(prepared) => prepared,
        Err(failure) => match recover(failure.recovery).await {
            Ok(()) => return Err(failure.message),
            Err(recovery_error) => {
                return Err(format!(
                    "{}; update_gateway_restore_failed: {recovery_error}",
                    failure.message
                ));
            }
        },
    };
    match install(package, bytes, &prepared) {
        Ok(()) => Ok(true),
        Err(install_error) => match recover(prepared).await {
            Ok(()) => Err(install_error),
            Err(recovery_error) => Err(format!(
                "{install_error}; update_gateway_restore_failed: {recovery_error}"
            )),
        },
    }
}
