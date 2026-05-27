use profiler_core::core_api::{CoreApi, CoreError};

#[test]
fn core_api_initialization_is_idempotent() {
    let first = CoreApi::initialize().expect("first initialization should succeed");
    let second = CoreApi::initialize().expect("second initialization should succeed");

    assert!(first.runtime_ready);
    assert!(second.runtime_ready);
}

#[test]
fn core_error_display_is_typed_and_readable() {
    let err = CoreError::InvalidInput("serial is required".to_string());

    assert_eq!(err.to_string(), "invalid input: serial is required");
}

#[test]
fn device_operations_validate_required_inputs_before_adb() {
    let _ = CoreApi::initialize().expect("runtime initialization should succeed");
    let rt = profiler_core::runtime();

    let err = rt
        .block_on(CoreApi::adb_shell("", "id"))
        .expect_err("empty serial must be rejected before adb");
    assert!(matches!(err, CoreError::InvalidInput(_)));

    let err = rt
        .block_on(CoreApi::adb_push("device", "", "/data/local/tmp/file"))
        .expect_err("empty local path must be rejected before adb");
    assert!(matches!(err, CoreError::InvalidInput(_)));

    let err = rt
        .block_on(CoreApi::connect("", 50051))
        .expect_err("empty serial must be rejected before connecting");
    assert!(matches!(err, CoreError::InvalidInput(_)));
}

#[test]
fn utility_operations_validate_required_inputs_before_daemon_access() {
    let _ = CoreApi::initialize().expect("runtime initialization should succeed");
    let rt = profiler_core::runtime();

    let err = rt
        .block_on(CoreApi::daemon_shell("", "id"))
        .expect_err("empty serial must be rejected before daemon access");
    assert!(matches!(err, CoreError::InvalidInput(_)));

    let err = rt
        .block_on(CoreApi::get_package_info("device", ""))
        .expect_err("empty package must be rejected before daemon access");
    assert!(matches!(err, CoreError::InvalidInput(_)));

    let err = rt
        .block_on(CoreApi::path_exists("device", ""))
        .expect_err("empty path must be rejected before daemon access");
    assert!(matches!(err, CoreError::InvalidInput(_)));

    let err = rt
        .block_on(CoreApi::input_text("device", ""))
        .expect_err("empty text must be rejected before daemon access");
    assert!(matches!(err, CoreError::InvalidInput(_)));

    let err = rt
        .block_on(CoreApi::screenshot("device", 85, ""))
        .expect_err("empty local path must be rejected before daemon access");
    assert!(matches!(err, CoreError::InvalidInput(_)));
}

#[test]
fn recording_operations_validate_required_inputs_before_daemon_access() {
    let _ = CoreApi::initialize().expect("runtime initialization should succeed");
    let rt = profiler_core::runtime();

    let err = rt
        .block_on(CoreApi::start_rtb_stream(
            "",
            123,
            1.0,
            "mperf",
            Default::default(),
        ))
        .expect_err("empty serial must be rejected before daemon access");
    assert!(matches!(err, CoreError::InvalidInput(_)));

    let err = rt
        .block_on(CoreApi::start_cr_stream(
            "device",
            0.0,
            &[0],
            false,
            false,
            false,
            &[],
        ))
        .expect_err("non-positive interval must be rejected before daemon access");
    assert!(matches!(err, CoreError::InvalidInput(_)));

    let err = rt
        .block_on(CoreApi::start_perfetto("device", 0, "", 10, "buffers {}"))
        .expect_err("empty mode must be rejected before daemon access");
    assert!(matches!(err, CoreError::InvalidInput(_)));

    let err = rt
        .block_on(CoreApi::pull_file("device", "", "/tmp/out"))
        .expect_err("empty remote path must be rejected before daemon access");
    assert!(matches!(err, CoreError::InvalidInput(_)));
}
