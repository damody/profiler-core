use thiserror::Error;

pub use crate::adb::DeviceInfo;
pub use crate::grpc::client::{
    GenericResult, PackageInfoResult, PerfettoStatusInfo, RtbStreamOptions, ShellResult, TopAppInfo,
};

pub type CoreResult<T> = Result<T, CoreError>;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("device not found: {0}")]
    DeviceNotFound(String),
    #[error("daemon not running: {0}")]
    DaemonNotRunning(String),
    #[error("operation failed: {0}")]
    Operation(String),
    #[error("runtime error: {0}")]
    Runtime(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuntimeStatus {
    pub freshly_initialized: bool,
    pub runtime_ready: bool,
}

pub struct CoreApi;

impl CoreApi {
    pub fn initialize() -> CoreResult<RuntimeStatus> {
        let freshly_initialized = crate::init_runtime();
        let runtime_ready = std::panic::catch_unwind(|| {
            let _ = crate::runtime();
        })
        .is_ok();

        if runtime_ready {
            Ok(RuntimeStatus {
                freshly_initialized,
                runtime_ready,
            })
        } else {
            Err(CoreError::Runtime(
                "profiler-core runtime was not initialized".to_string(),
            ))
        }
    }

    pub fn shutdown() {
        crate::shutdown_runtime();
    }

    pub async fn list_devices() -> CoreResult<Vec<DeviceInfo>> {
        crate::adb::devices::get_devices()
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn adb_shell(serial: &str, command: &str) -> CoreResult<String> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("command", command)?;
        crate::adb::commands::shell(serial, command)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn adb_push(serial: &str, local: &str, remote: &str) -> CoreResult<()> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("local path", local)?;
        validate_non_empty("remote path", remote)?;
        crate::adb::commands::push(serial, local, remote)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn adb_pull(serial: &str, remote: &str, local: &str) -> CoreResult<()> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("remote path", remote)?;
        validate_non_empty("local path", local)?;
        crate::adb::commands::pull(serial, remote, local)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn adb_root(serial: &str) -> CoreResult<String> {
        validate_non_empty("serial", serial)?;
        crate::adb::commands::root(serial)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn adb_remount(serial: &str) -> CoreResult<String> {
        validate_non_empty("serial", serial)?;
        crate::adb::commands::remount(serial)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn wifi_adb_connect(serial: &str, device_ip: &str, port: u16) -> CoreResult<String> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("device ip", device_ip)?;
        validate_port(port)?;

        let _ = crate::adb::commands::disconnect_all().await;
        crate::adb::commands::tcpip(serial, port)
            .await
            .map_err(CoreError::from_anyhow)?;
        crate::adb::commands::connect_device(device_ip, port)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn wifi_adb_disconnect(serial: &str) -> CoreResult<String> {
        validate_non_empty("serial", serial)?;
        crate::adb::commands::disconnect(serial)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn connect(serial: &str, port: u16) -> CoreResult<()> {
        validate_non_empty("serial", serial)?;
        validate_port(port)?;
        Self::initialize()?;
        ensure_device_present(serial).await?;

        crate::adb::commands::forward(serial, port, 50051)
            .await
            .map_err(CoreError::from_anyhow)?;
        let addr = format!("http://127.0.0.1:{port}");
        let client = crate::grpc::client::ProfilerClient::connect(&addr)
            .await
            .map_err(|err| CoreError::DaemonNotRunning(err.to_string()))?;
        let entry = crate::ConnectionEntry {
            serial: serial.to_string(),
            client,
            port,
            daemon_low_overhead: false,
        };
        crate::connections()
            .lock()
            .insert(serial.to_string(), entry);
        Ok(())
    }

    pub async fn deploy_and_connect(
        serial: &str,
        local_path: Option<&str>,
        remote_path: &str,
        port: u16,
        daemon_low_overhead: bool,
    ) -> CoreResult<()> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("remote path", remote_path)?;
        validate_port(port)?;
        Self::initialize()?;
        ensure_device_present(serial).await?;

        let local_path = local_path.unwrap_or_default();
        crate::adb::daemon::ensure_running_rooted(
            serial,
            local_path,
            remote_path,
            port,
            daemon_low_overhead,
        )
        .await
        .map_err(CoreError::from_anyhow)?;

        if !crate::adb::daemon::is_running(serial).await {
            return Err(CoreError::DaemonNotRunning(
                "realtime_profile did not stay alive".to_string(),
            ));
        }

        let daemon_port = crate::adb::daemon::get_grpc_port(serial)
            .await
            .unwrap_or(port);
        crate::adb::commands::forward(serial, port, daemon_port)
            .await
            .map_err(CoreError::from_anyhow)?;
        if daemon_low_overhead {
            match (port.checked_add(1), daemon_port.checked_add(1)) {
                (Some(local_sync_port), Some(device_sync_port)) => {
                    crate::adb::commands::forward(serial, local_sync_port, device_sync_port)
                        .await
                        .map_err(CoreError::from_anyhow)?;
                }
                _ => {
                    return Err(CoreError::InvalidInput(
                        "sync control port overflow".to_string(),
                    ))
                }
            }
        }

        let addr = format!("http://127.0.0.1:{port}");
        if daemon_low_overhead {
            if let Some(sync_addr) = sync_control_addr_for_port(port) {
                if crate::grpc::sync_control::health(&sync_addr).is_ok() {
                    let client = crate::grpc::client::ProfilerClient::connect_lazy(&addr)
                        .map_err(CoreError::from_anyhow)?;
                    let entry = crate::ConnectionEntry {
                        serial: serial.to_string(),
                        client,
                        port,
                        daemon_low_overhead,
                    };
                    crate::connections()
                        .lock()
                        .insert(serial.to_string(), entry);
                    return Ok(());
                }
            }
        }

        let client = crate::grpc::client::ProfilerClient::connect(&addr)
            .await
            .map_err(|err| CoreError::DaemonNotRunning(err.to_string()))?;

        let entry = crate::ConnectionEntry {
            serial: serial.to_string(),
            client,
            port,
            daemon_low_overhead,
        };
        crate::connections()
            .lock()
            .insert(serial.to_string(), entry);
        Ok(())
    }

    pub fn disconnect(serial: &str) -> CoreResult<()> {
        validate_non_empty("serial", serial)?;
        Self::initialize()?;
        crate::connections()
            .lock()
            .remove(serial)
            .map(|_| ())
            .ok_or_else(|| CoreError::DeviceNotFound(serial.to_string()))
    }

    pub fn is_connected(serial: &str) -> CoreResult<bool> {
        validate_non_empty("serial", serial)?;
        Self::initialize()?;
        Ok(crate::connections().lock().contains_key(serial))
    }

    pub async fn health_check(serial: &str) -> CoreResult<HealthInfo> {
        validate_non_empty("serial", serial)?;
        Self::initialize()?;
        let (mut client, port, daemon_low_overhead) = {
            let conns = crate::connections().lock();
            let entry = conns
                .get(serial)
                .ok_or_else(|| CoreError::DeviceNotFound(serial.to_string()))?;
            (entry.client.clone(), entry.port, entry.daemon_low_overhead)
        };

        if daemon_low_overhead {
            if let Some(sync_addr) = sync_control_addr_for_port(port) {
                if let Ok((version, status)) = crate::grpc::sync_control::health(&sync_addr) {
                    return Ok(HealthInfo { version, status });
                }
            }
        }

        let (version, status) = client
            .health()
            .await
            .map_err(|err| CoreError::DaemonNotRunning(err.to_string()))?;
        Ok(HealthInfo { version, status })
    }

    pub async fn top_app(serial: &str) -> CoreResult<TopAppInfo> {
        validate_non_empty("serial", serial)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .get_top_app()
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn get_pid(serial: &str, package_name: &str) -> CoreResult<i32> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("package name", package_name)?;
        let mut client = connected_client(serial)?;
        client
            .get_pid(package_name)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn list_packages(
        serial: &str,
        third_party_only: bool,
    ) -> CoreResult<Vec<PackageInfoResult>> {
        validate_non_empty("serial", serial)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .list_packages(third_party_only)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn get_package_info(
        serial: &str,
        package_name: &str,
    ) -> CoreResult<PackageInfoResult> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("package name", package_name)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .get_package_info(package_name)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn launch_app(serial: &str, package_name: &str) -> CoreResult<GenericResult> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("package name", package_name)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .launch_app(package_name)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn stop_app(serial: &str, package_name: &str) -> CoreResult<GenericResult> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("package name", package_name)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .stop_app(package_name)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn install_apk(serial: &str, remote_apk_path: &str) -> CoreResult<GenericResult> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("remote apk path", remote_apk_path)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .install_apk(remote_apk_path)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn daemon_shell(serial: &str, command: &str) -> CoreResult<ShellResult> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("command", command)?;
        let mut client = connected_client(serial)?;
        client.shell(command).await.map_err(CoreError::from_anyhow)
    }

    pub async fn set_device_id(serial: &str, device_id: &str) -> CoreResult<GenericResult> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("device id", device_id)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .set_device_id(device_id)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn screen_size(serial: &str) -> CoreResult<(i32, i32)> {
        validate_non_empty("serial", serial)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .get_screen_size()
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn screenshot(serial: &str, quality: i32, local_path: &str) -> CoreResult<()> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("local path", local_path)?;
        let mut client = connected_client(serial)?;
        client
            .screenshot(quality, local_path, |_current, _total| {})
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn input_tap(serial: &str, x: i32, y: i32) -> CoreResult<GenericResult> {
        validate_non_empty("serial", serial)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .input_tap(x, y)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn input_swipe(
        serial: &str,
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        duration_ms: i32,
    ) -> CoreResult<GenericResult> {
        validate_non_empty("serial", serial)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .input_swipe(x1, y1, x2, y2, duration_ms)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn input_text(serial: &str, text: &str) -> CoreResult<GenericResult> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("text", text)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .input_text(text)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn input_key_event(serial: &str, key_code: i32) -> CoreResult<GenericResult> {
        validate_non_empty("serial", serial)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .input_key_event(key_code)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn get_device_prop(serial: &str, prop_name: &str) -> CoreResult<String> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("property name", prop_name)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .get_device_prop(prop_name)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn path_exists(serial: &str, path: &str) -> CoreResult<bool> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("path", path)?;
        let mut client = connected_client(serial)?;
        client
            .path_exists(path)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn temperature(serial: &str) -> CoreResult<(f64, f64, u32)> {
        validate_non_empty("serial", serial)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .get_temperature()
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn set_charging(serial: &str, enable: bool) -> CoreResult<GenericResult> {
        validate_non_empty("serial", serial)?;
        let mut client = connected_client(serial)?;
        client
            .set_charging(enable)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn remove_file(serial: &str, path: &str) -> CoreResult<GenericResult> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("path", path)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .remove_file(path)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn create_archive(
        serial: &str,
        working_directory: &str,
        target: &str,
        output_path: &str,
    ) -> CoreResult<GenericResult> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("working directory", working_directory)?;
        validate_non_empty("target", target)?;
        validate_non_empty("output path", output_path)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .create_archive(working_directory, target, output_path)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn extract_archive(
        serial: &str,
        working_directory: &str,
        archive_path: &str,
    ) -> CoreResult<GenericResult> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("working directory", working_directory)?;
        validate_non_empty("archive path", archive_path)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .extract_archive(working_directory, archive_path)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn chmod(
        serial: &str,
        path: &str,
        mode: &str,
        recursive: bool,
    ) -> CoreResult<GenericResult> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("path", path)?;
        validate_non_empty("mode", mode)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .chmod(path, mode, recursive)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn chown(
        serial: &str,
        path: &str,
        uid: i32,
        gid: i32,
        recursive: bool,
    ) -> CoreResult<GenericResult> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("path", path)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .chown(path, uid, gid, recursive)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn get_file_owner(serial: &str, path: &str) -> CoreResult<i32> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("path", path)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .get_file_owner(path)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn start_rtb_stream(
        serial: &str,
        pid: i32,
        interval_secs: f64,
        mode: &str,
        options: RtbStreamOptions,
    ) -> CoreResult<tonic::Streaming<crate::proto::RtbDataPoint>> {
        validate_non_empty("serial", serial)?;
        validate_positive_f64("interval_secs", interval_secs)?;
        let mut client = connected_client(serial)?;
        client
            .start_rtb_stream(pid, interval_secs, mode, options)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn start_cr_stream(
        serial: &str,
        interval_secs: f64,
        cpus: &[i32],
        exclude_kernel: bool,
        diff_kernel: bool,
        full_mode: bool,
        custom_events: &[u32],
    ) -> CoreResult<tonic::Streaming<crate::proto::CrDataPoint>> {
        validate_non_empty("serial", serial)?;
        validate_positive_f64("interval_secs", interval_secs)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .start_cr_stream(
                interval_secs,
                cpus,
                exclude_kernel,
                diff_kernel,
                full_mode,
                custom_events,
            )
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn start_tc_stream(
        serial: &str,
        pid: i32,
        interval_secs: f64,
        exclude_kernel: bool,
        diff_kernel: bool,
        top_threads_count: i32,
        full_mode: bool,
        custom_events: &[u32],
    ) -> CoreResult<tonic::Streaming<crate::proto::TcDataPoint>> {
        validate_non_empty("serial", serial)?;
        validate_positive_f64("interval_secs", interval_secs)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .start_tc_stream(
                pid,
                interval_secs,
                exclude_kernel,
                diff_kernel,
                top_threads_count,
                full_mode,
                custom_events,
            )
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn start_cml_stream(
        serial: &str,
        cpus: &[i32],
        max_footprint_kb: u32,
        min_footprint_kb: u32,
    ) -> CoreResult<tonic::Streaming<crate::proto::CmlDataPoint>> {
        validate_non_empty("serial", serial)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .start_cml_stream(cpus, max_footprint_kb, min_footprint_kb)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn start_gc_stream(
        serial: &str,
        gpu_device_number: u32,
        interval_secs: f64,
        counter_ids: &[u32],
    ) -> CoreResult<tonic::Streaming<crate::proto::GcDataPoint>> {
        validate_non_empty("serial", serial)?;
        validate_positive_f64("interval_secs", interval_secs)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .start_gc_stream(gpu_device_number, interval_secs, counter_ids)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn start_perfetto(
        serial: &str,
        pid: i32,
        mode: &str,
        duration_secs: i32,
        config_pbtxt: &str,
    ) -> CoreResult<crate::proto::PerfettoResponse> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("mode", mode)?;
        validate_positive_i32("duration_secs", duration_secs)?;
        validate_non_empty("config pbtxt", config_pbtxt)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .start_perfetto(pid, mode, duration_secs, config_pbtxt)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn perfetto_status(serial: &str) -> CoreResult<PerfettoStatusInfo> {
        validate_non_empty("serial", serial)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .get_perfetto_status()
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn pull_file(serial: &str, remote_path: &str, local_path: &str) -> CoreResult<()> {
        validate_non_empty("serial", serial)?;
        validate_non_empty("remote path", remote_path)?;
        validate_non_empty("local path", local_path)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .pull_file(remote_path, local_path, |_current, _total| {})
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn stop_recording(
        serial: &str,
        session_type: &str,
    ) -> CoreResult<crate::proto::StopResponse> {
        validate_non_empty("serial", serial)?;
        let mut client = connected_client(serial)?;
        client
            .stop_recording(session_type)
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn rtb_summary(serial: &str) -> CoreResult<crate::proto::RtbSummary> {
        validate_non_empty("serial", serial)?;
        let mut client = connected_client(serial)?;
        client
            .get_rtb_summary()
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn discover_ftrace_events(
        serial: &str,
    ) -> CoreResult<crate::proto::DiscoverFtraceResponse> {
        validate_non_empty("serial", serial)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .discover_ftrace_events()
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn discover_pmu_events(
        serial: &str,
    ) -> CoreResult<crate::proto::DiscoverPmuResponse> {
        validate_non_empty("serial", serial)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .discover_pmu_events()
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn pmu_hw_counters(serial: &str) -> CoreResult<crate::proto::PmuHwCounterResponse> {
        validate_non_empty("serial", serial)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .get_pmu_hw_counters()
            .await
            .map_err(CoreError::from_anyhow)
    }

    pub async fn discover_gpu_counters(
        serial: &str,
    ) -> CoreResult<crate::proto::GcDiscoverResponse> {
        validate_non_empty("serial", serial)?;
        let mut conns = connected_clients();
        let entry = connected_entry(&mut conns, serial)?;
        entry
            .client
            .discover_gpu_counters()
            .await
            .map_err(CoreError::from_anyhow)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthInfo {
    pub version: String,
    pub status: String,
}

impl CoreError {
    fn from_anyhow(err: anyhow::Error) -> Self {
        Self::Operation(err.to_string())
    }
}

async fn ensure_device_present(serial: &str) -> CoreResult<()> {
    let devices = CoreApi::list_devices().await?;
    if devices
        .iter()
        .any(|device| device.serial == serial && device.state == "device")
    {
        Ok(())
    } else {
        Err(CoreError::DeviceNotFound(serial.to_string()))
    }
}

fn connected_clients(
) -> parking_lot::MutexGuard<'static, std::collections::HashMap<String, crate::ConnectionEntry>> {
    let _ = CoreApi::initialize();
    crate::connections().lock()
}

fn connected_client(serial: &str) -> CoreResult<crate::grpc::client::ProfilerClient> {
    let mut conns = connected_clients();
    let entry = connected_entry(&mut conns, serial)?;
    Ok(entry.client.clone())
}

fn connected_entry<'a>(
    conns: &'a mut std::collections::HashMap<String, crate::ConnectionEntry>,
    serial: &str,
) -> CoreResult<&'a mut crate::ConnectionEntry> {
    conns
        .get_mut(serial)
        .ok_or_else(|| CoreError::DeviceNotFound(serial.to_string()))
}

fn sync_control_addr_for_port(port: u16) -> Option<String> {
    port.checked_add(1).map(|port| format!("127.0.0.1:{port}"))
}

fn validate_non_empty(field: &str, value: &str) -> CoreResult<()> {
    if value.trim().is_empty() {
        Err(CoreError::InvalidInput(format!("{field} is required")))
    } else {
        Ok(())
    }
}

fn validate_port(port: u16) -> CoreResult<()> {
    if port == 0 {
        Err(CoreError::InvalidInput(
            "port must be greater than zero".to_string(),
        ))
    } else {
        Ok(())
    }
}

fn validate_positive_f64(field: &str, value: f64) -> CoreResult<()> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(CoreError::InvalidInput(format!(
            "{field} must be greater than zero"
        )))
    }
}

fn validate_positive_i32(field: &str, value: i32) -> CoreResult<()> {
    if value > 0 {
        Ok(())
    } else {
        Err(CoreError::InvalidInput(format!(
            "{field} must be greater than zero"
        )))
    }
}
