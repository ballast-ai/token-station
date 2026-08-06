use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use token_station_desktop_lib::desktop_update::{
    check_with, install_with, DesktopUpdateOperation, DesktopUpdateStatus,
};

#[test]
fn concurrent_update_operations_are_rejected_until_the_first_finishes() {
    let operation = DesktopUpdateOperation::default();
    let first = operation.try_begin().expect("first operation starts");
    assert_eq!(
        operation.try_begin().err().as_deref(),
        Some("update_in_progress: 已有更新任务正在进行")
    );
    drop(first);
    assert!(operation.try_begin().is_ok());
}

#[tokio::test]
async fn a_source_build_without_the_official_public_key_never_contacts_the_update_endpoint() {
    let contacted = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&contacted);

    let view = check_with("1.1.2", "", move || async move {
        observed.store(true, Ordering::SeqCst);
        Ok(None)
    })
    .await;

    assert_eq!(view.status, DesktopUpdateStatus::Unsupported);
    assert_eq!(view.current_version, "1.1.2");
    assert_eq!(view.version, None);
    assert!(view
        .message
        .as_deref()
        .unwrap_or_default()
        .contains("官方更新公钥"));
    assert!(!contacted.load(Ordering::SeqCst));
}

#[tokio::test]
async fn a_download_or_signature_failure_never_stops_the_gateway_or_installs() {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));

    let check_events = Arc::clone(&events);
    let download_events = Arc::clone(&events);
    let prepare_events = Arc::clone(&events);
    let install_events = Arc::clone(&events);
    let result = install_with(
        "official-public-key",
        move || async move {
            check_events.lock().unwrap().push("check");
            Ok(Some("signed-package"))
        },
        move |package| async move {
            download_events.lock().unwrap().push("download");
            assert_eq!(package, "signed-package");
            Err("更新包签名校验失败".to_owned())
        },
        move || async move {
            prepare_events.lock().unwrap().push("prepare");
            Ok(())
        },
        move |_package, _bytes, _prepared| {
            install_events.lock().unwrap().push("install");
            Ok(())
        },
        |_prepared| async { Ok(()) },
    )
    .await;

    assert_eq!(result.unwrap_err(), "更新包签名校验失败");
    assert_eq!(*events.lock().unwrap(), vec!["check", "download"]);
}

#[tokio::test]
async fn a_verified_update_stops_the_gateway_only_before_installing() {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));

    let check_events = Arc::clone(&events);
    let download_events = Arc::clone(&events);
    let prepare_events = Arc::clone(&events);
    let install_events = Arc::clone(&events);
    let result = install_with(
        "official-public-key",
        move || async move {
            check_events.lock().unwrap().push("check");
            Ok(Some("signed-package"))
        },
        move |package| async move {
            download_events.lock().unwrap().push("download-and-verify");
            Ok((package, b"verified-bytes".to_vec()))
        },
        move || async move {
            prepare_events.lock().unwrap().push("prepare-gateway");
            Ok("gateway-was-running")
        },
        move |package, bytes, prepared| {
            install_events.lock().unwrap().push("install");
            assert_eq!(package, "signed-package");
            assert_eq!(bytes, b"verified-bytes");
            assert_eq!(*prepared, "gateway-was-running");
            Ok(())
        },
        |_prepared| async { Ok(()) },
    )
    .await;

    assert_eq!(result, Ok(true));
    assert_eq!(
        *events.lock().unwrap(),
        vec!["check", "download-and-verify", "prepare-gateway", "install"]
    );
}

#[tokio::test]
async fn a_failed_install_recovers_a_previously_running_gateway() {
    let events = Arc::new(std::sync::Mutex::new(Vec::new()));
    let prepare_events = Arc::clone(&events);
    let install_events = Arc::clone(&events);
    let recover_events = Arc::clone(&events);

    let result = install_with(
        "official-public-key",
        || async { Ok(Some("signed-package")) },
        |package| async move { Ok((package, b"verified-bytes".to_vec())) },
        move || async move {
            prepare_events.lock().unwrap().push("prepare-gateway");
            Ok(true)
        },
        move |_package, _bytes, was_active| {
            install_events.lock().unwrap().push("install-failed");
            assert!(*was_active);
            Err("安装器拒绝替换".to_owned())
        },
        move |was_active| async move {
            assert!(was_active);
            recover_events.lock().unwrap().push("restore-gateway");
            Ok(())
        },
    )
    .await;

    assert_eq!(result.unwrap_err(), "安装器拒绝替换");
    assert_eq!(
        *events.lock().unwrap(),
        vec!["prepare-gateway", "install-failed", "restore-gateway"]
    );
}
