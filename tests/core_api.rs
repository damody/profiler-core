use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;

use profiler_core::core_api::{CoreApi, CoreError};
use prost::Message;

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

#[test]
fn rtb_stream_allows_empty_legacy_full_mode() {
    let _ = CoreApi::initialize().expect("runtime initialization should succeed");
    let rt = profiler_core::runtime();

    let err = rt
        .block_on(CoreApi::start_rtb_stream(
            "missing-device",
            123,
            1.0,
            "",
            Default::default(),
        ))
        .expect_err("missing device should fail after mode validation");

    assert!(matches!(err, CoreError::DeviceNotFound(_)));
}

#[test]
fn low_overhead_health_check_uses_sync_control_port() {
    let _ = CoreApi::initialize().expect("runtime initialization should succeed");
    profiler_core::connections().lock().clear();

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake sync health server");
    let sync_port = listener.local_addr().unwrap().port();
    assert!(sync_port > 1);
    let base_port = sync_port - 1;
    let handle = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("accept sync health request");
        let mut opcode = [0u8; 1];
        stream.read_exact(&mut opcode).unwrap();
        assert_eq!(opcode[0], 2);

        let mut len = [0u8; 4];
        stream.read_exact(&mut len).unwrap();
        let request_len = u32::from_be_bytes(len) as usize;
        let mut request = vec![0u8; request_len];
        stream.read_exact(&mut request).unwrap();

        let response = profiler_core::proto::HealthResponse {
            version: "sync-v1".to_string(),
            status: "ok".to_string(),
        };
        let bytes = response.encode_to_vec();
        stream.write_all(&[0]).unwrap();
        stream
            .write_all(&(bytes.len() as u32).to_be_bytes())
            .unwrap();
        stream.write_all(&bytes).unwrap();
    });

    let serial = "low-overhead-sync-health";
    let client = profiler_core::runtime()
        .block_on(async {
            profiler_core::grpc::client::ProfilerClient::connect_lazy(&format!(
                "http://127.0.0.1:{base_port}"
            ))
        })
        .unwrap();
    profiler_core::connections().lock().insert(
        serial.to_string(),
        profiler_core::ConnectionEntry {
            serial: serial.to_string(),
            client,
            port: base_port,
            daemon_low_overhead: true,
        },
    );

    let health = profiler_core::runtime()
        .block_on(CoreApi::health_check(serial))
        .expect("low-overhead health should use sync control");

    assert_eq!(health.version, "sync-v1");
    assert_eq!(health.status, "ok");
    handle.join().unwrap();
    profiler_core::connections().lock().remove(serial);
}
