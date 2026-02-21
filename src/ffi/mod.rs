pub mod types;
pub mod utils;

use std::ffi::c_void;
use std::ptr;

use crate::adb;
use crate::grpc;
use crate::ffi::types::*;
use crate::ffi::utils::*;

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

/// Initialise the profiler runtime.  Must be called before any other function.
#[no_mangle]
pub extern "C" fn profiler_init() -> ProfilerResult {
    use log4rs::append::file::FileAppender;
    use log4rs::config::{Appender, Config, Root};
    use log4rs::encode::pattern::PatternEncoder;

    let logfile = FileAppender::builder()
        .encoder(Box::new(PatternEncoder::new(
            "{d(%Y-%m-%d %H:%M:%S%.3f)} [{l}] {m}{n}",
        )))
        .build("profiler_core.log");

    if let Ok(appender) = logfile {
        let config = Config::builder()
            .appender(Appender::builder().build("file", Box::new(appender)))
            .build(
                Root::builder()
                    .appender("file")
                    .build(log::LevelFilter::Info),
            );
        if let Ok(cfg) = config {
            let _ = log4rs::init_config(cfg);
        }
    }

    crate::init_runtime();
    log::info!("profiler_init complete");
    ProfilerResult::Ok
}

/// Shut down the profiler runtime and release all resources.
#[no_mangle]
pub extern "C" fn profiler_shutdown() {
    crate::shutdown_runtime();
}

/// Retrieve the last error message as a UTF-16 string.
///
/// Returns a pointer to a null-terminated UTF-16 string.  The caller must
/// free it with `profiler_free_string`.  Returns null if no error is stored.
#[no_mangle]
pub extern "C" fn profiler_get_last_error() -> *mut u16 {
    let msg = crate::take_last_error();
    if msg.is_empty() {
        return ptr::null_mut();
    }
    to_wide_ptr(&msg)
}

// ---------------------------------------------------------------------------
// Device enumeration
// ---------------------------------------------------------------------------

/// Enumerate connected ADB devices.
///
/// On success `out` is populated with a device list.
/// The caller must free it with `profiler_free_devices`.
#[no_mangle]
pub extern "C" fn profiler_get_devices(out: *mut ProfilerDeviceList) -> ProfilerResult {
    if out.is_null() {
        return ProfilerResult::InvalidParameter;
    }

    let rt = crate::runtime();
    let result = rt.block_on(adb::devices::get_devices());

    match result {
        Ok(devices) => {
            let count = devices.len();
            let mut ffi_devices: Vec<ProfilerDevice> = devices
                .iter()
                .map(|d| ProfilerDevice {
                    serial: to_wide_ptr(&d.serial),
                    model: to_wide_ptr(&d.model),
                    state: to_wide_ptr(&d.state),
                })
                .collect();

            let ptr = ffi_devices.as_mut_ptr();
            std::mem::forget(ffi_devices);

            unsafe {
                (*out).devices = ptr;
                (*out).count = count;
            }
            ProfilerResult::Ok
        }
        Err(e) => {
            log::error!("profiler_get_devices failed: {e:#}");
            unsafe {
                (*out).devices = ptr::null_mut();
                (*out).count = 0;
            }
            ProfilerResult::OperationFailed
        }
    }
}

/// Free a device list previously returned by `profiler_get_devices`.
#[no_mangle]
pub extern "C" fn profiler_free_devices(list: *mut ProfilerDeviceList) {
    if list.is_null() {
        return;
    }
    unsafe {
        let count = (*list).count;
        let devices_ptr = (*list).devices;
        if !devices_ptr.is_null() && count > 0 {
            let devices = Vec::from_raw_parts(devices_ptr, count, count);
            for d in devices {
                free_wide_ptr(d.serial);
                free_wide_ptr(d.model);
                free_wide_ptr(d.state);
            }
        }
        (*list).devices = ptr::null_mut();
        (*list).count = 0;
    }
}

// ---------------------------------------------------------------------------
// Connection management
// ---------------------------------------------------------------------------

/// Connect to the realtime_profile daemon on the given device using port 50051.
#[no_mangle]
pub extern "C" fn profiler_connect(serial: *const u16) -> ProfilerResult {
    profiler_connect_with_port(serial, 50051)
}

/// Connect to the realtime_profile daemon on the given device using a custom port.
///
/// This sets up adb port forwarding and establishes a gRPC channel.
#[no_mangle]
pub extern "C" fn profiler_connect_with_port(serial: *const u16, port: u16) -> ProfilerResult {
    if serial.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let serial_str = unsafe { from_wide_ptr(serial) };
    if serial_str.is_empty() {
        return ProfilerResult::InvalidParameter;
    }

    let rt = crate::runtime();

    // Check the device exists first
    let devices = match rt.block_on(adb::devices::get_devices()) {
        Ok(d) => d,
        Err(e) => {
            log::error!("profiler_connect: cannot list devices: {e:#}");
            return ProfilerResult::OperationFailed;
        }
    };
    if !devices.iter().any(|d| d.serial == serial_str && d.state == "device") {
        return ProfilerResult::DeviceNotFound;
    }

    // Set up port forwarding (local port → device port 50051)
    if let Err(e) = rt.block_on(adb::commands::forward(&serial_str, port, 50051)) {
        log::error!("profiler_connect: adb forward failed: {e:#}");
        return ProfilerResult::OperationFailed;
    }

    // Try to connect to the gRPC daemon
    let addr = format!("http://127.0.0.1:{port}");
    match rt.block_on(grpc::client::ProfilerClient::connect(&addr)) {
        Ok(client) => {
            let entry = crate::ConnectionEntry {
                serial: serial_str.clone(),
                client,
                port,
            };
            crate::connections().lock().insert(serial_str, entry);
            ProfilerResult::Ok
        }
        Err(e) => {
            log::error!("profiler_connect: gRPC connect failed: {e:#}");
            ProfilerResult::DaemonNotRunning
        }
    }
}

/// Deploy (if needed) and connect to the realtime_profile daemon.
///
/// 1. Check if daemon is already running (`pidof`).
/// 2. If not running: push `local_path` → `remote_path` (skip push if `local_path`
///    is null/empty), then start daemon with `--daemon --grpc-port`.
/// 3. Set up `adb forward` and establish gRPC channel.
///
/// `port` is the **local** TCP port used for adb forward; the daemon always
/// listens on the same port on the device side.
#[no_mangle]
pub extern "C" fn profiler_deploy_and_connect(
    serial: *const u16,
    local_path: *const u16,
    remote_path: *const u16,
    port: u16,
) -> ProfilerResult {
    if serial.is_null() || remote_path.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let serial_str = unsafe { from_wide_ptr(serial) };
    let remote_str = unsafe { from_wide_ptr(remote_path) };
    if serial_str.is_empty() || remote_str.is_empty() {
        return ProfilerResult::InvalidParameter;
    }
    let local_str = if local_path.is_null() {
        String::new()
    } else {
        unsafe { from_wide_ptr(local_path) }
    };

    let rt = crate::runtime();

    // Check device exists
    let devices = match rt.block_on(adb::devices::get_devices()) {
        Ok(d) => d,
        Err(e) => {
            log::error!("profiler_deploy_and_connect: cannot list devices: {e:#}");
            return ProfilerResult::OperationFailed;
        }
    };
    if !devices.iter().any(|d| d.serial == serial_str && d.state == "device") {
        return ProfilerResult::DeviceNotFound;
    }

    // Deploy + start if daemon is not running
    if !rt.block_on(adb::daemon::is_running(&serial_str)) {
        log::info!("Daemon not running on {serial_str}, deploying...");
        if let Err(e) = rt.block_on(adb::daemon::deploy_and_start(
            &serial_str,
            &local_str,
            &remote_str,
            port,
        )) {
            log::error!("profiler_deploy_and_connect: deploy failed: {e:#}");
            return ProfilerResult::OperationFailed;
        }

        // Verify it started
        if !rt.block_on(adb::daemon::is_running(&serial_str)) {
            log::error!("profiler_deploy_and_connect: daemon did not stay alive");
            return ProfilerResult::DaemonNotRunning;
        }
    }

    // Port forwarding (local port → device port)
    // If the daemon was already running (e.g. started via USB), it may listen
    // on a different port than `port`.  Detect the actual port from cmdline.
    let daemon_port = rt
        .block_on(adb::daemon::get_grpc_port(&serial_str))
        .unwrap_or(port);
    log::info!(
        "profiler_deploy_and_connect: requested port={port}, daemon_port={daemon_port}"
    );
    if let Err(e) = rt.block_on(adb::commands::forward(&serial_str, port, daemon_port)) {
        log::error!("profiler_deploy_and_connect: adb forward failed: {e:#}");
        return ProfilerResult::OperationFailed;
    }

    // gRPC connect
    let addr = format!("http://127.0.0.1:{port}");
    match rt.block_on(grpc::client::ProfilerClient::connect(&addr)) {
        Ok(client) => {
            let entry = crate::ConnectionEntry {
                serial: serial_str.clone(),
                client,
                port,
            };
            crate::connections().lock().insert(serial_str, entry);
            ProfilerResult::Ok
        }
        Err(e) => {
            log::error!("profiler_deploy_and_connect: gRPC connect failed: {e:#}");
            ProfilerResult::DaemonNotRunning
        }
    }
}

/// Disconnect from the device's daemon.
#[no_mangle]
pub extern "C" fn profiler_disconnect(serial: *const u16) -> ProfilerResult {
    if serial.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let serial_str = unsafe { from_wide_ptr(serial) };

    match crate::connections().lock().remove(&serial_str) {
        Some(_entry) => {
            log::info!("Disconnected from {serial_str}");
            ProfilerResult::Ok
        }
        None => ProfilerResult::DeviceNotFound,
    }
}

/// Check whether we currently hold an active connection for the device.
#[no_mangle]
pub extern "C" fn profiler_is_connected(serial: *const u16) -> bool {
    if serial.is_null() {
        return false;
    }
    let serial_str = unsafe { from_wide_ptr(serial) };
    crate::connections().lock().contains_key(&serial_str)
}

// ---------------------------------------------------------------------------
// Health check
// ---------------------------------------------------------------------------

/// Perform a health check against the daemon on the given device.
///
/// On success `out` is populated with version and status strings.
/// The caller must free them with `profiler_free_health_info`.
#[no_mangle]
pub extern "C" fn profiler_health_check(
    serial: *const u16,
    out: *mut ProfilerHealthInfo,
) -> ProfilerResult {
    if serial.is_null() || out.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let serial_str = unsafe { from_wide_ptr(serial) };
    let rt = crate::runtime();

    let mut conns = crate::connections().lock();
    let entry = match conns.get_mut(&serial_str) {
        Some(e) => e,
        None => return ProfilerResult::DeviceNotFound,
    };

    match rt.block_on(entry.client.health()) {
        Ok((version, status)) => {
            unsafe {
                (*out).version = to_wide_ptr(&version);
                (*out).status = to_wide_ptr(&status);
            }
            ProfilerResult::Ok
        }
        Err(e) => {
            log::error!("profiler_health_check: {e:#}");
            // Connection is likely dead — remove it
            let serial_clone = serial_str.clone();
            drop(conns);
            crate::connections().lock().remove(&serial_clone);
            ProfilerResult::DaemonNotRunning
        }
    }
}

/// Free a `ProfilerHealthInfo` returned by `profiler_health_check`.
#[no_mangle]
pub extern "C" fn profiler_free_health_info(info: *mut ProfilerHealthInfo) {
    if info.is_null() {
        return;
    }
    unsafe {
        free_wide_ptr((*info).version);
        free_wide_ptr((*info).status);
        (*info).version = ptr::null_mut();
        (*info).status = ptr::null_mut();
    }
}

// ---------------------------------------------------------------------------
// Top app / PID
// ---------------------------------------------------------------------------

/// Query the foreground app on the device via gRPC.
#[no_mangle]
pub extern "C" fn profiler_get_top_app(
    serial: *const u16,
    out: *mut ProfilerTopApp,
) -> ProfilerResult {
    if serial.is_null() || out.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let serial_str = unsafe { from_wide_ptr(serial) };
    let rt = crate::runtime();

    let mut conns = crate::connections().lock();
    let entry = match conns.get_mut(&serial_str) {
        Some(e) => e,
        None => return ProfilerResult::DeviceNotFound,
    };

    match rt.block_on(entry.client.get_top_app()) {
        Ok(info) => {
            unsafe {
                (*out).package_name = to_wide_ptr(&info.package_name);
                (*out).activity = to_wide_ptr(&info.activity);
                (*out).pid = info.pid;
            }
            ProfilerResult::Ok
        }
        Err(e) => {
            log::error!("profiler_get_top_app: {e:#}");
            ProfilerResult::OperationFailed
        }
    }
}

/// Free a `ProfilerTopApp` returned by `profiler_get_top_app`.
#[no_mangle]
pub extern "C" fn profiler_free_top_app(app: *mut ProfilerTopApp) {
    if app.is_null() {
        return;
    }
    unsafe {
        free_wide_ptr((*app).package_name);
        free_wide_ptr((*app).activity);
        (*app).package_name = ptr::null_mut();
        (*app).activity = ptr::null_mut();
    }
}

/// Get the PID of a package by name.
#[no_mangle]
pub extern "C" fn profiler_get_pid(
    serial: *const u16,
    package: *const u16,
    pid_out: *mut i32,
) -> ProfilerResult {
    if serial.is_null() || package.is_null() || pid_out.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let serial_str = unsafe { from_wide_ptr(serial) };
    let package_str = unsafe { from_wide_ptr(package) };
    let rt = crate::runtime();

    let mut conns = crate::connections().lock();
    let entry = match conns.get_mut(&serial_str) {
        Some(e) => e,
        None => return ProfilerResult::DeviceNotFound,
    };

    match rt.block_on(entry.client.get_pid(&package_str)) {
        Ok(pid) => {
            unsafe { *pid_out = pid; }
            ProfilerResult::Ok
        }
        Err(e) => {
            log::error!("profiler_get_pid: {e:#}");
            ProfilerResult::OperationFailed
        }
    }
}

// ---------------------------------------------------------------------------
// RTB streaming
// ---------------------------------------------------------------------------

/// Start a real-time benchmark stream.
///
/// On success, `handle_out` receives an opaque handle id.  Use
/// `profiler_poll_rtb` to read data and `profiler_stop_rtb` to stop.
#[no_mangle]
pub extern "C" fn profiler_start_rtb(
    serial: *const u16,
    pid: i32,
    interval_secs: f64,
    mode: *const u16,
    handle_out: *mut u64,
) -> ProfilerResult {
    if serial.is_null() || handle_out.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let serial_str = unsafe { from_wide_ptr(serial) };
    let mode_str = if mode.is_null() {
        String::new()
    } else {
        unsafe { from_wide_ptr(mode) }
    };
    let rt = crate::runtime();

    let mut conns = crate::connections().lock();
    let entry = match conns.get_mut(&serial_str) {
        Some(e) => e,
        None => return ProfilerResult::DeviceNotFound,
    };

    match rt.block_on(entry.client.start_rtb_stream(pid, interval_secs, &mode_str)) {
        Ok(stream) => {
            let handle_id = crate::next_rtb_handle_id();
            let handle = grpc::streaming::RtbStreamHandle::start(rt, stream, 512);
            crate::rtb_handles().lock().insert(handle_id, handle);
            unsafe { *handle_out = handle_id; }
            ProfilerResult::Ok
        }
        Err(e) => {
            log::error!("profiler_start_rtb: {e:#}");
            ProfilerResult::OperationFailed
        }
    }
}

/// Non-blocking poll for the next RTB data point.
///
/// Returns `true` if data was available and `out` was populated.
/// Returns `false` if no data is available yet (caller should try again).
#[no_mangle]
pub extern "C" fn profiler_poll_rtb(handle: u64, out: *mut ProfilerRtbData) -> bool {
    if out.is_null() {
        return false;
    }

    let handles = crate::rtb_handles().lock();
    let h = match handles.get(&handle) {
        Some(h) => h,
        None => return false,
    };

    match h.poll() {
        Some(dp) => {
            // Copy cpu_freqs
            let (freqs_ptr, freqs_count) = if dp.cpu_freqs_mhz.is_empty() {
                (ptr::null_mut(), 0)
            } else {
                let mut v = dp.cpu_freqs_mhz.clone();
                let ptr = v.as_mut_ptr();
                let len = v.len();
                std::mem::forget(v);
                (ptr, len)
            };

            // Copy cpu_usages
            let (usages_ptr, usages_count) = if dp.cpu_usages_pct.is_empty() {
                (ptr::null_mut(), 0)
            } else {
                let mut v = dp.cpu_usages_pct.clone();
                let ptr = v.as_mut_ptr();
                let len = v.len();
                std::mem::forget(v);
                (ptr, len)
            };

            // Copy frame_times
            let (ft_ptr, ft_count) = if dp.frame_times_ms.is_empty() {
                (ptr::null_mut(), 0)
            } else {
                let mut v = dp.frame_times_ms.clone();
                let ptr = v.as_mut_ptr();
                let len = v.len();
                std::mem::forget(v);
                (ptr, len)
            };

            unsafe {
                (*out).timestamp_ms = dp.timestamp_ms;
                (*out).fps = dp.fps;
                (*out).power_mw = dp.power_mw;
                (*out).power_ma = dp.power_ma;
                (*out).voltage_v = dp.voltage_v;
                (*out).board_temp_c = dp.board_temp_c;
                (*out).battery_temp_c = dp.battery_temp_c;
                (*out).gpu_freq_mhz = dp.gpu_freq_mhz;
                (*out).gpu_loading_pct = dp.gpu_loading_pct;
                (*out).bcpu_freq_mhz = dp.bcpu_freq_mhz;
                (*out).bcpu_usage_pct = dp.bcpu_usage_pct;
                (*out).mcpu_freq_mhz = dp.mcpu_freq_mhz;
                (*out).mcpu_usage_pct = dp.mcpu_usage_pct;
                (*out).lcpu_freq_mhz = dp.lcpu_freq_mhz;
                (*out).lcpu_usage_pct = dp.lcpu_usage_pct;
                (*out).cpu_freqs_mhz = freqs_ptr;
                (*out).cpu_freqs_count = freqs_count;
                (*out).cpu_usages_pct = usages_ptr;
                (*out).cpu_usages_count = usages_count;
                (*out).total_mips = dp.total_mips;
                (*out).game_mips = dp.game_mips;
                (*out).logical_mips = dp.logical_mips;
                (*out).render_mips = dp.render_mips;
                (*out).rhi_mips = dp.rhi_mips;
                (*out).dsu_freq_mhz = dp.dsu_freq_mhz;
                (*out).dram_freq_mhz = dp.dram_freq_mhz;
                (*out).vcore_v = dp.vcore_v;
                (*out).wss_kb = dp.wss_kb;
                (*out).pss_kb = dp.pss_kb;
                (*out).frame_times_ms = ft_ptr;
                (*out).frame_times_count = ft_count;
                (*out).cpu_time_ms = dp.cpu_time_ms;
                (*out).gpu_time_ms = dp.gpu_time_ms;
                (*out).power_avg_mw = dp.power_avg_mw;
                (*out).process_name = if dp.process_name.is_empty() {
                    ptr::null_mut()
                } else {
                    to_wide_ptr(&dp.process_name)
                };
            }
            true
        }
        None => false,
    }
}

/// Stop an RTB stream and release the handle.
#[no_mangle]
pub extern "C" fn profiler_stop_rtb(handle: u64) -> ProfilerResult {
    match crate::rtb_handles().lock().remove(&handle) {
        Some(h) => {
            h.cancel();
            ProfilerResult::Ok
        }
        None => ProfilerResult::InvalidParameter,
    }
}

/// Free the dynamic arrays inside a `ProfilerRtbData`.
#[no_mangle]
pub extern "C" fn profiler_free_rtb_data(data: *mut ProfilerRtbData) {
    if data.is_null() {
        return;
    }
    unsafe {
        let freqs_ptr = (*data).cpu_freqs_mhz;
        let freqs_count = (*data).cpu_freqs_count;
        if !freqs_ptr.is_null() && freqs_count > 0 {
            drop(Vec::from_raw_parts(freqs_ptr, freqs_count, freqs_count));
        }
        (*data).cpu_freqs_mhz = ptr::null_mut();
        (*data).cpu_freqs_count = 0;

        let usages_ptr = (*data).cpu_usages_pct;
        let usages_count = (*data).cpu_usages_count;
        if !usages_ptr.is_null() && usages_count > 0 {
            drop(Vec::from_raw_parts(usages_ptr, usages_count, usages_count));
        }
        (*data).cpu_usages_pct = ptr::null_mut();
        (*data).cpu_usages_count = 0;

        let ft_ptr = (*data).frame_times_ms;
        let ft_count = (*data).frame_times_count;
        if !ft_ptr.is_null() && ft_count > 0 {
            drop(Vec::from_raw_parts(ft_ptr, ft_count, ft_count));
        }
        (*data).frame_times_ms = ptr::null_mut();
        (*data).frame_times_count = 0;

        free_wide_ptr((*data).process_name);
        (*data).process_name = ptr::null_mut();
    }
}

// ---------------------------------------------------------------------------
// Perfetto
// ---------------------------------------------------------------------------

/// Start a Perfetto trace session.
///
/// `config_pbtxt` may be null — when non-null and non-empty it overrides `mode`.
#[no_mangle]
pub extern "C" fn profiler_start_perfetto(
    serial: *const u16,
    pid: i32,
    mode: *const u16,
    duration_secs: i32,
    config_pbtxt: *const u16,
) -> ProfilerResult {
    if serial.is_null() || mode.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let serial_str = unsafe { from_wide_ptr(serial) };
    let mode_str = unsafe { from_wide_ptr(mode) };
    let pbtxt_str = if config_pbtxt.is_null() {
        String::new()
    } else {
        unsafe { from_wide_ptr(config_pbtxt) }
    };
    let rt = crate::runtime();

    let mut conns = crate::connections().lock();
    let entry = match conns.get_mut(&serial_str) {
        Some(e) => e,
        None => return ProfilerResult::DeviceNotFound,
    };

    match rt.block_on(entry.client.start_perfetto(pid, &mode_str, duration_secs, &pbtxt_str)) {
        Ok(resp) => {
            if resp.success {
                ProfilerResult::Ok
            } else {
                log::error!("profiler_start_perfetto: daemon said: {}", resp.message);
                ProfilerResult::OperationFailed
            }
        }
        Err(e) => {
            log::error!("profiler_start_perfetto: {e:#}");
            ProfilerResult::OperationFailed
        }
    }
}

/// Get the current Perfetto trace status.
#[no_mangle]
pub extern "C" fn profiler_get_perfetto_status(
    serial: *const u16,
    out: *mut ProfilerPerfettoStatus,
) -> ProfilerResult {
    if serial.is_null() || out.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let serial_str = unsafe { from_wide_ptr(serial) };
    let rt = crate::runtime();

    let mut conns = crate::connections().lock();
    let entry = match conns.get_mut(&serial_str) {
        Some(e) => e,
        None => return ProfilerResult::DeviceNotFound,
    };

    match rt.block_on(entry.client.get_perfetto_status()) {
        Ok(status) => {
            unsafe {
                (*out).state = status.state;
                (*out).progress_pct = status.progress_pct;
                (*out).output_path = to_wide_ptr(&status.output_path);
            }
            ProfilerResult::Ok
        }
        Err(e) => {
            log::error!("profiler_get_perfetto_status: {e:#}");
            ProfilerResult::OperationFailed
        }
    }
}

/// Free a `ProfilerPerfettoStatus`.
#[no_mangle]
pub extern "C" fn profiler_free_perfetto_status(status: *mut ProfilerPerfettoStatus) {
    if status.is_null() {
        return;
    }
    unsafe {
        free_wide_ptr((*status).output_path);
        (*status).output_path = ptr::null_mut();
    }
}

// ---------------------------------------------------------------------------
// File pull
// ---------------------------------------------------------------------------

/// Pull a file from the device using the gRPC PullFile streaming RPC.
///
/// If `cb` is provided it will be called with progress updates.
#[no_mangle]
pub extern "C" fn profiler_pull_file(
    serial: *const u16,
    remote: *const u16,
    local: *const u16,
    cb: Option<ProgressCallback>,
    user_data: *mut c_void,
) -> ProfilerResult {
    if serial.is_null() || remote.is_null() || local.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let serial_str = unsafe { from_wide_ptr(serial) };
    let remote_str = unsafe { from_wide_ptr(remote) };
    let local_str = unsafe { from_wide_ptr(local) };
    let rt = crate::runtime();

    let mut conns = crate::connections().lock();
    let entry = match conns.get_mut(&serial_str) {
        Some(e) => e,
        None => return ProfilerResult::DeviceNotFound,
    };

    // Wrap user_data in a Send-safe wrapper for the async block
    let user_data_val = user_data as usize;

    match rt.block_on(entry.client.pull_file(&remote_str, &local_str, move |done, total| {
        if let Some(callback) = cb {
            callback(done, total, user_data_val as *mut c_void);
        }
    })) {
        Ok(()) => ProfilerResult::Ok,
        Err(e) => {
            log::error!("profiler_pull_file: {e:#}");
            ProfilerResult::OperationFailed
        }
    }
}

// ---------------------------------------------------------------------------
// Stop recording
// ---------------------------------------------------------------------------

/// Tell the daemon to stop the current recording session.
#[no_mangle]
pub extern "C" fn profiler_stop_recording(serial: *const u16) -> ProfilerResult {
    if serial.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let serial_str = unsafe { from_wide_ptr(serial) };
    let rt = crate::runtime();

    let mut conns = crate::connections().lock();
    let entry = match conns.get_mut(&serial_str) {
        Some(e) => e,
        None => return ProfilerResult::DeviceNotFound,
    };

    match rt.block_on(entry.client.stop_recording()) {
        Ok(_) => ProfilerResult::Ok,
        Err(e) => {
            log::error!("profiler_stop_recording: {e:#}");
            ProfilerResult::OperationFailed
        }
    }
}

// ---------------------------------------------------------------------------
// Generic ADB shell (legacy — still calls adb.exe directly)
// ---------------------------------------------------------------------------

/// Execute an arbitrary `adb shell` command and return stdout as a wide string.
///
/// On success `*out` is set to a newly-allocated wide string that the caller
/// must free with `profiler_free_string`.
#[no_mangle]
pub extern "C" fn profiler_adb_shell(
    serial: *const u16,
    command: *const u16,
    out: *mut *mut u16,
) -> ProfilerResult {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if serial.is_null() || command.is_null() || out.is_null() {
            return ProfilerResult::InvalidParameter;
        }
        let serial_str = unsafe { from_wide_ptr(serial) };
        let command_str = unsafe { from_wide_ptr(command) };
        let rt = crate::runtime();

        match rt.block_on(adb::commands::shell(&serial_str, &command_str)) {
            Ok(output) => {
                unsafe { *out = to_wide_ptr(&output); }
                ProfilerResult::Ok
            }
            Err(e) => {
                log::error!("profiler_adb_shell: {e:#}");
                unsafe { *out = ptr::null_mut(); }
                ProfilerResult::OperationFailed
            }
        }
    })) {
        Ok(r) => r,
        Err(e) => {
            let msg = if let Some(s) = e.downcast_ref::<&str>() {
                format!("profiler_adb_shell panicked: {s}")
            } else if let Some(s) = e.downcast_ref::<String>() {
                format!("profiler_adb_shell panicked: {s}")
            } else {
                "profiler_adb_shell panicked (unknown payload)".to_string()
            };
            log::error!("{msg}");
            crate::set_last_error(&msg);
            ProfilerResult::OperationFailed
        }
    }
}

/// Free a wide string returned by `profiler_adb_shell`.
#[no_mangle]
pub extern "C" fn profiler_free_string(ptr: *mut u16) {
    unsafe { free_wide_ptr(ptr); }
}

// ---------------------------------------------------------------------------
// WiFi ADB
// ---------------------------------------------------------------------------

/// Connect to the device over WiFi ADB.
///
/// 1. Disconnect all existing WiFi ADB connections.
/// 2. Switch the device to TCP/IP mode (`adb tcpip`).
/// 3. Wait 2 seconds for adbd to restart.
/// 4. Connect via `adb connect ip:port`.
///
/// On success, `*out` is set to a newly-allocated wide string with the
/// connection result.  Caller must free with `profiler_free_string`.
#[no_mangle]
pub extern "C" fn profiler_wifi_adb_connect(
    serial: *const u16,
    device_ip: *const u16,
    port: u16,
    out: *mut *mut u16,
) -> ProfilerResult {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if serial.is_null() || device_ip.is_null() || out.is_null() {
            return ProfilerResult::InvalidParameter;
        }
        let serial_str = unsafe { from_wide_ptr(serial) };
        let ip_str = unsafe { from_wide_ptr(device_ip) };
        log::info!("profiler_wifi_adb_connect: serial={serial_str}, ip={ip_str}, port={port}");
        if serial_str.is_empty() || ip_str.is_empty() {
            return ProfilerResult::InvalidParameter;
        }

        let rt = crate::runtime();

        // 1. Disconnect all (best-effort)
        log::info!("profiler_wifi_adb_connect: step 1 — disconnect_all");
        let _ = rt.block_on(adb::commands::disconnect_all());

        // 2. Switch to tcpip mode
        log::info!("profiler_wifi_adb_connect: step 2 — tcpip {port}");
        if let Err(e) = rt.block_on(adb::commands::tcpip(&serial_str, port)) {
            let msg = format!("adb tcpip failed: {e:#}");
            log::error!("profiler_wifi_adb_connect: {msg}");
            crate::set_last_error(&msg);
            return ProfilerResult::OperationFailed;
        }

        // 3. Wait for adbd to restart
        log::info!("profiler_wifi_adb_connect: step 3 — sleep 2s");
        std::thread::sleep(std::time::Duration::from_secs(2));

        // 4. Connect
        log::info!("profiler_wifi_adb_connect: step 4 — connect {ip_str}:{port}");
        match rt.block_on(adb::commands::connect_device(&ip_str, port)) {
            Ok(result) => {
                log::info!("profiler_wifi_adb_connect: success — {result}");
                unsafe { *out = to_wide_ptr(&result); }
                ProfilerResult::Ok
            }
            Err(e) => {
                let msg = format!("adb connect failed: {e:#}");
                log::error!("profiler_wifi_adb_connect: {msg}");
                crate::set_last_error(&msg);
                unsafe { *out = ptr::null_mut(); }
                ProfilerResult::OperationFailed
            }
        }
    })) {
        Ok(r) => r,
        Err(e) => {
            let msg = if let Some(s) = e.downcast_ref::<&str>() {
                format!("profiler_wifi_adb_connect panicked: {s}")
            } else if let Some(s) = e.downcast_ref::<String>() {
                format!("profiler_wifi_adb_connect panicked: {s}")
            } else {
                "profiler_wifi_adb_connect panicked (unknown payload)".to_string()
            };
            log::error!("{msg}");
            crate::set_last_error(&msg);
            ProfilerResult::OperationFailed
        }
    }
}

/// Disconnect a specific WiFi ADB device.
///
/// On success, `*out` is set to a newly-allocated wide string with the
/// disconnect result.  Caller must free with `profiler_free_string`.
#[no_mangle]
pub extern "C" fn profiler_wifi_adb_disconnect(
    serial: *const u16,
    out: *mut *mut u16,
) -> ProfilerResult {
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        if serial.is_null() || out.is_null() {
            return ProfilerResult::InvalidParameter;
        }
        let serial_str = unsafe { from_wide_ptr(serial) };
        log::info!("profiler_wifi_adb_disconnect: serial={serial_str}");
        if serial_str.is_empty() {
            return ProfilerResult::InvalidParameter;
        }

        let rt = crate::runtime();

        match rt.block_on(adb::commands::disconnect(&serial_str)) {
            Ok(result) => {
                log::info!("profiler_wifi_adb_disconnect: success — {result}");
                unsafe { *out = to_wide_ptr(&result); }
                ProfilerResult::Ok
            }
            Err(e) => {
                let msg = format!("adb disconnect failed: {e:#}");
                log::error!("profiler_wifi_adb_disconnect: {msg}");
                crate::set_last_error(&msg);
                unsafe { *out = ptr::null_mut(); }
                ProfilerResult::OperationFailed
            }
        }
    })) {
        Ok(r) => r,
        Err(e) => {
            let msg = if let Some(s) = e.downcast_ref::<&str>() {
                format!("profiler_wifi_adb_disconnect panicked: {s}")
            } else if let Some(s) = e.downcast_ref::<String>() {
                format!("profiler_wifi_adb_disconnect panicked: {s}")
            } else {
                "profiler_wifi_adb_disconnect panicked (unknown payload)".to_string()
            };
            log::error!("{msg}");
            crate::set_last_error(&msg);
            ProfilerResult::OperationFailed
        }
    }
}

// ===========================================================================
// New gRPC-backed FFI functions
// ===========================================================================

// ---------------------------------------------------------------------------
// Package Management
// ---------------------------------------------------------------------------

/// Helper: get connection entry and runtime, returns result via closure.
/// This avoids repeating boilerplate in every FFI function.
macro_rules! with_connection {
    ($serial:expr, |$rt:ident, $entry:ident| $body:expr) => {{
        if $serial.is_null() {
            return ProfilerResult::InvalidParameter;
        }
        let serial_str = unsafe { from_wide_ptr($serial) };
        let $rt = crate::runtime();

        let mut conns = crate::connections().lock();
        let $entry = match conns.get_mut(&serial_str) {
            Some(e) => e,
            None => return ProfilerResult::DeviceNotFound,
        };

        $body
    }};
}

/// List installed packages.
#[no_mangle]
pub extern "C" fn profiler_list_packages(
    serial: *const u16,
    third_party_only: bool,
    out: *mut ProfilerPackageList,
) -> ProfilerResult {
    if out.is_null() {
        return ProfilerResult::InvalidParameter;
    }

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.list_packages(third_party_only)) {
            Ok(packages) => {
                let count = packages.len();
                let mut ffi_packages: Vec<ProfilerPackageInfo> = packages
                    .iter()
                    .map(|p| ProfilerPackageInfo {
                        package_name: to_wide_ptr(&p.package_name),
                        apk_path: to_wide_ptr(&p.apk_path),
                        version_name: to_wide_ptr(&p.version_name),
                        version_code: p.version_code,
                        pid: p.pid,
                    })
                    .collect();

                let pkg_ptr = ffi_packages.as_mut_ptr();
                std::mem::forget(ffi_packages);

                unsafe {
                    (*out).packages = pkg_ptr;
                    (*out).count = count;
                }
                ProfilerResult::Ok
            }
            Err(e) => {
                log::error!("profiler_list_packages: {e:#}");
                unsafe {
                    (*out).packages = ptr::null_mut();
                    (*out).count = 0;
                }
                ProfilerResult::OperationFailed
            }
        }
    })
}

/// Free a package list.
#[no_mangle]
pub extern "C" fn profiler_free_package_list(list: *mut ProfilerPackageList) {
    if list.is_null() {
        return;
    }
    unsafe {
        let count = (*list).count;
        let packages_ptr = (*list).packages;
        if !packages_ptr.is_null() && count > 0 {
            let packages = Vec::from_raw_parts(packages_ptr, count, count);
            for p in packages {
                free_wide_ptr(p.package_name);
                free_wide_ptr(p.apk_path);
                free_wide_ptr(p.version_name);
            }
        }
        (*list).packages = ptr::null_mut();
        (*list).count = 0;
    }
}

/// Get detailed info for a single package.
#[no_mangle]
pub extern "C" fn profiler_get_package_info(
    serial: *const u16,
    package: *const u16,
    out: *mut ProfilerPackageInfo,
) -> ProfilerResult {
    if package.is_null() || out.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let package_str = unsafe { from_wide_ptr(package) };

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.get_package_info(&package_str)) {
            Ok(info) => {
                unsafe {
                    (*out).package_name = to_wide_ptr(&info.package_name);
                    (*out).apk_path = to_wide_ptr(&info.apk_path);
                    (*out).version_name = to_wide_ptr(&info.version_name);
                    (*out).version_code = info.version_code;
                    (*out).pid = info.pid;
                }
                ProfilerResult::Ok
            }
            Err(e) => {
                log::error!("profiler_get_package_info: {e:#}");
                ProfilerResult::OperationFailed
            }
        }
    })
}

/// Free a single package info struct.
#[no_mangle]
pub extern "C" fn profiler_free_package_info(info: *mut ProfilerPackageInfo) {
    if info.is_null() {
        return;
    }
    unsafe {
        free_wide_ptr((*info).package_name);
        free_wide_ptr((*info).apk_path);
        free_wide_ptr((*info).version_name);
        (*info).package_name = ptr::null_mut();
        (*info).apk_path = ptr::null_mut();
        (*info).version_name = ptr::null_mut();
    }
}

/// Launch an app by package name.
#[no_mangle]
pub extern "C" fn profiler_launch_app(
    serial: *const u16,
    package: *const u16,
) -> ProfilerResult {
    if package.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let package_str = unsafe { from_wide_ptr(package) };

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.launch_app(&package_str)) {
            Ok(resp) => {
                if resp.success { ProfilerResult::Ok } else {
                    log::error!("profiler_launch_app: {}", resp.message);
                    ProfilerResult::OperationFailed
                }
            }
            Err(e) => {
                log::error!("profiler_launch_app: {e:#}");
                ProfilerResult::OperationFailed
            }
        }
    })
}

/// Force-stop an app.
#[no_mangle]
pub extern "C" fn profiler_stop_app(
    serial: *const u16,
    package: *const u16,
) -> ProfilerResult {
    if package.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let package_str = unsafe { from_wide_ptr(package) };

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.stop_app(&package_str)) {
            Ok(resp) => {
                if resp.success { ProfilerResult::Ok } else {
                    log::error!("profiler_stop_app: {}", resp.message);
                    ProfilerResult::OperationFailed
                }
            }
            Err(e) => {
                log::error!("profiler_stop_app: {e:#}");
                ProfilerResult::OperationFailed
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Shell (via gRPC daemon)
// ---------------------------------------------------------------------------

/// Execute a shell command on the device via the gRPC daemon (root).
#[no_mangle]
pub extern "C" fn profiler_shell(
    serial: *const u16,
    command: *const u16,
    out: *mut ProfilerShellResult,
) -> ProfilerResult {
    if command.is_null() || out.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let command_str = unsafe { from_wide_ptr(command) };

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.shell(&command_str)) {
            Ok(result) => {
                unsafe {
                    (*out).exit_code = result.exit_code;
                    (*out).stdout = to_wide_ptr(&result.stdout);
                    (*out).stderr = to_wide_ptr(&result.stderr);
                }
                ProfilerResult::Ok
            }
            Err(e) => {
                log::error!("profiler_shell: {e:#}");
                unsafe {
                    (*out).exit_code = -1;
                    (*out).stdout = ptr::null_mut();
                    (*out).stderr = ptr::null_mut();
                }
                ProfilerResult::OperationFailed
            }
        }
    })
}

/// Free a shell result.
#[no_mangle]
pub extern "C" fn profiler_free_shell_result(result: *mut ProfilerShellResult) {
    if result.is_null() {
        return;
    }
    unsafe {
        free_wide_ptr((*result).stdout);
        free_wide_ptr((*result).stderr);
        (*result).stdout = ptr::null_mut();
        (*result).stderr = ptr::null_mut();
    }
}

// ---------------------------------------------------------------------------
// Screen & Input
// ---------------------------------------------------------------------------

/// Get the device screen size.
#[no_mangle]
pub extern "C" fn profiler_get_screen_size(
    serial: *const u16,
    width_out: *mut i32,
    height_out: *mut i32,
) -> ProfilerResult {
    if width_out.is_null() || height_out.is_null() {
        return ProfilerResult::InvalidParameter;
    }

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.get_screen_size()) {
            Ok((w, h)) => {
                unsafe {
                    *width_out = w;
                    *height_out = h;
                }
                ProfilerResult::Ok
            }
            Err(e) => {
                log::error!("profiler_get_screen_size: {e:#}");
                ProfilerResult::OperationFailed
            }
        }
    })
}

/// Take a screenshot, streaming JPEG bytes to a local file.
#[no_mangle]
pub extern "C" fn profiler_screenshot(
    serial: *const u16,
    quality: i32,
    local_path: *const u16,
    cb: Option<ProgressCallback>,
    user_data: *mut c_void,
) -> ProfilerResult {
    if local_path.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let local_str = unsafe { from_wide_ptr(local_path) };
    let user_data_val = user_data as usize;

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.screenshot(quality, &local_str, move |done, total| {
            if let Some(callback) = cb {
                callback(done, total, user_data_val as *mut c_void);
            }
        })) {
            Ok(()) => ProfilerResult::Ok,
            Err(e) => {
                log::error!("profiler_screenshot: {e:#}");
                ProfilerResult::OperationFailed
            }
        }
    })
}

/// Tap at the given coordinates.
#[no_mangle]
pub extern "C" fn profiler_input_tap(
    serial: *const u16,
    x: i32,
    y: i32,
) -> ProfilerResult {
    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.input_tap(x, y)) {
            Ok(_) => ProfilerResult::Ok,
            Err(e) => {
                log::error!("profiler_input_tap: {e:#}");
                ProfilerResult::OperationFailed
            }
        }
    })
}

/// Swipe between two points.
#[no_mangle]
pub extern "C" fn profiler_input_swipe(
    serial: *const u16,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    duration_ms: i32,
) -> ProfilerResult {
    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.input_swipe(x1, y1, x2, y2, duration_ms)) {
            Ok(_) => ProfilerResult::Ok,
            Err(e) => {
                log::error!("profiler_input_swipe: {e:#}");
                ProfilerResult::OperationFailed
            }
        }
    })
}

/// Input text on the device.
#[no_mangle]
pub extern "C" fn profiler_input_text(
    serial: *const u16,
    text: *const u16,
) -> ProfilerResult {
    if text.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let text_str = unsafe { from_wide_ptr(text) };

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.input_text(&text_str)) {
            Ok(_) => ProfilerResult::Ok,
            Err(e) => {
                log::error!("profiler_input_text: {e:#}");
                ProfilerResult::OperationFailed
            }
        }
    })
}

/// Send a key event.
#[no_mangle]
pub extern "C" fn profiler_input_key_event(
    serial: *const u16,
    key_code: i32,
) -> ProfilerResult {
    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.input_key_event(key_code)) {
            Ok(_) => ProfilerResult::Ok,
            Err(e) => {
                log::error!("profiler_input_key_event: {e:#}");
                ProfilerResult::OperationFailed
            }
        }
    })
}

// ---------------------------------------------------------------------------
// File Push
// ---------------------------------------------------------------------------

/// Push a local file to the device via gRPC client streaming.
#[no_mangle]
pub extern "C" fn profiler_push_file(
    serial: *const u16,
    local: *const u16,
    remote: *const u16,
    cb: Option<ProgressCallback>,
    user_data: *mut c_void,
) -> ProfilerResult {
    if local.is_null() || remote.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let local_str = unsafe { from_wide_ptr(local) };
    let remote_str = unsafe { from_wide_ptr(remote) };
    let user_data_val = user_data as usize;

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.push_file(&local_str, &remote_str, move |done, total| {
            if let Some(callback) = cb {
                callback(done, total, user_data_val as *mut c_void);
            }
        })) {
            Ok(_) => ProfilerResult::Ok,
            Err(e) => {
                log::error!("profiler_push_file: {e:#}");
                ProfilerResult::OperationFailed
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Device Utilities
// ---------------------------------------------------------------------------

/// Get a device property value (e.g. "ro.hardware").
#[no_mangle]
pub extern "C" fn profiler_get_device_prop(
    serial: *const u16,
    prop: *const u16,
    out: *mut *mut u16,
) -> ProfilerResult {
    if prop.is_null() || out.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let prop_str = unsafe { from_wide_ptr(prop) };

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.get_device_prop(&prop_str)) {
            Ok(value) => {
                unsafe { *out = to_wide_ptr(&value); }
                ProfilerResult::Ok
            }
            Err(e) => {
                log::error!("profiler_get_device_prop: {e:#}");
                unsafe { *out = ptr::null_mut(); }
                ProfilerResult::OperationFailed
            }
        }
    })
}

/// Check if a path exists on the device.
#[no_mangle]
pub extern "C" fn profiler_path_exists(
    serial: *const u16,
    path: *const u16,
    out: *mut bool,
) -> ProfilerResult {
    if path.is_null() || out.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let path_str = unsafe { from_wide_ptr(path) };

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.path_exists(&path_str)) {
            Ok(exists) => {
                unsafe { *out = exists; }
                ProfilerResult::Ok
            }
            Err(e) => {
                log::error!("profiler_path_exists: {e:#}");
                unsafe { *out = false; }
                ProfilerResult::OperationFailed
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Temperature
// ---------------------------------------------------------------------------

/// Get battery and board temperatures from the device.
#[no_mangle]
pub extern "C" fn profiler_get_temperature(
    serial: *const u16,
    out: *mut ProfilerTemperature,
) -> ProfilerResult {
    if out.is_null() {
        return ProfilerResult::InvalidParameter;
    }

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.get_temperature()) {
            Ok((battery, board, level)) => {
                unsafe {
                    (*out).battery_temp_c = battery;
                    (*out).board_temp_c = board;
                    (*out).battery_level_pct = level;
                }
                ProfilerResult::Ok
            }
            Err(e) => {
                log::error!("profiler_get_temperature: {e:#}");
                ProfilerResult::OperationFailed
            }
        }
    })
}

// ---------------------------------------------------------------------------
// RTB Summary
// ---------------------------------------------------------------------------

/// Helper: convert a proto ThreadSnapshot into an FFI struct.
fn thread_snapshot_to_ffi(ts: &crate::proto::ThreadSnapshot) -> ProfilerThreadSnapshot {
    ProfilerThreadSnapshot {
        tid: ts.tid,
        tgid: ts.tgid,
        name: to_wide_ptr(&ts.name),
        loading_pct: ts.loading_pct,
        c0_pct: ts.c0_pct,
        c1_pct: ts.c1_pct,
        c2_pct: ts.c2_pct,
        runnable_pct: ts.runnable_pct,
        mips: ts.mips,
        mcps: ts.mcps,
        cpi: ts.cpi,
    }
}

/// Helper: create an empty FFI thread snapshot.
fn empty_thread_snapshot() -> ProfilerThreadSnapshot {
    ProfilerThreadSnapshot {
        tid: 0,
        tgid: 0,
        name: ptr::null_mut(),
        loading_pct: 0.0,
        c0_pct: 0.0,
        c1_pct: 0.0,
        c2_pct: 0.0,
        runnable_pct: 0.0,
        mips: 0.0,
        mcps: 0.0,
        cpi: 0.0,
    }
}

/// Get the RTB summary (post-recording statistics).
#[no_mangle]
pub extern "C" fn profiler_get_rtb_summary(
    serial: *const u16,
    out: *mut ProfilerRtbSummary,
) -> ProfilerResult {
    if out.is_null() {
        return ProfilerResult::InvalidParameter;
    }

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.get_rtb_summary()) {
            Ok(summary) => {
                // Convert top_threads
                let threads_count = summary.top_threads.len();
                let mut ffi_threads: Vec<ProfilerThreadSnapshot> = summary
                    .top_threads
                    .iter()
                    .map(|ts| thread_snapshot_to_ffi(ts))
                    .collect();
                let threads_ptr = if threads_count > 0 {
                    let p = ffi_threads.as_mut_ptr();
                    std::mem::forget(ffi_threads);
                    p
                } else {
                    ptr::null_mut()
                };

                // Convert freq_distributions
                let dists_count = summary.freq_distributions.len();
                let mut ffi_dists: Vec<ProfilerFreqDistribution> = summary
                    .freq_distributions
                    .iter()
                    .map(|fd| {
                        let buckets_count = fd.buckets.len();
                        let mut ffi_buckets: Vec<ProfilerFreqBucket> = fd
                            .buckets
                            .iter()
                            .map(|b| ProfilerFreqBucket {
                                freq_mhz: b.freq_mhz,
                                count: b.count,
                                percentage: b.percentage,
                            })
                            .collect();
                        let buckets_ptr = if buckets_count > 0 {
                            let p = ffi_buckets.as_mut_ptr();
                            std::mem::forget(ffi_buckets);
                            p
                        } else {
                            ptr::null_mut()
                        };
                        ProfilerFreqDistribution {
                            component: to_wide_ptr(&fd.component),
                            buckets: buckets_ptr,
                            buckets_count,
                        }
                    })
                    .collect();
                let dists_ptr = if dists_count > 0 {
                    let p = ffi_dists.as_mut_ptr();
                    std::mem::forget(ffi_dists);
                    p
                } else {
                    ptr::null_mut()
                };

                let logical = summary
                    .logical_thread
                    .as_ref()
                    .map(|ts| thread_snapshot_to_ffi(ts))
                    .unwrap_or_else(empty_thread_snapshot);
                let render = summary
                    .render_thread
                    .as_ref()
                    .map(|ts| thread_snapshot_to_ffi(ts))
                    .unwrap_or_else(empty_thread_snapshot);
                let rhi = summary
                    .rhi_thread
                    .as_ref()
                    .map(|ts| thread_snapshot_to_ffi(ts))
                    .unwrap_or_else(empty_thread_snapshot);

                unsafe {
                    (*out).top_threads = threads_ptr;
                    (*out).top_threads_count = threads_count;
                    (*out).freq_distributions = dists_ptr;
                    (*out).freq_distributions_count = dists_count;
                    (*out).logical_thread = logical;
                    (*out).render_thread = render;
                    (*out).rhi_thread = rhi;
                    (*out).start_temp = summary.start_temp;
                    (*out).end_temp = summary.end_temp;
                }
                ProfilerResult::Ok
            }
            Err(e) => {
                log::error!("profiler_get_rtb_summary: {e:#}");
                ProfilerResult::OperationFailed
            }
        }
    })
}

/// Free a thread snapshot's name string.
unsafe fn free_thread_snapshot(ts: &mut ProfilerThreadSnapshot) {
    free_wide_ptr(ts.name);
    ts.name = ptr::null_mut();
}

/// Free an RTB summary returned by `profiler_get_rtb_summary`.
#[no_mangle]
pub extern "C" fn profiler_free_rtb_summary(summary: *mut ProfilerRtbSummary) {
    if summary.is_null() {
        return;
    }
    unsafe {
        // Free top_threads
        let threads_ptr = (*summary).top_threads;
        let threads_count = (*summary).top_threads_count;
        if !threads_ptr.is_null() && threads_count > 0 {
            let mut threads = Vec::from_raw_parts(threads_ptr, threads_count, threads_count);
            for ts in threads.iter_mut() {
                free_thread_snapshot(ts);
            }
        }
        (*summary).top_threads = ptr::null_mut();
        (*summary).top_threads_count = 0;

        // Free freq_distributions
        let dists_ptr = (*summary).freq_distributions;
        let dists_count = (*summary).freq_distributions_count;
        if !dists_ptr.is_null() && dists_count > 0 {
            let mut dists = Vec::from_raw_parts(dists_ptr, dists_count, dists_count);
            for fd in dists.iter_mut() {
                free_wide_ptr(fd.component);
                fd.component = ptr::null_mut();
                if !fd.buckets.is_null() && fd.buckets_count > 0 {
                    drop(Vec::from_raw_parts(fd.buckets, fd.buckets_count, fd.buckets_count));
                }
                fd.buckets = ptr::null_mut();
                fd.buckets_count = 0;
            }
        }
        (*summary).freq_distributions = ptr::null_mut();
        (*summary).freq_distributions_count = 0;

        // Free game task thread snapshots
        free_thread_snapshot(&mut (*summary).logical_thread);
        free_thread_snapshot(&mut (*summary).render_thread);
        free_thread_snapshot(&mut (*summary).rhi_thread);
    }
}

/// Install an APK already on the device.
#[no_mangle]
pub extern "C" fn profiler_install_apk(
    serial: *const u16,
    remote_apk_path: *const u16,
) -> ProfilerResult {
    if remote_apk_path.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let path_str = unsafe { from_wide_ptr(remote_apk_path) };

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.install_apk(&path_str)) {
            Ok(resp) => {
                if resp.success { ProfilerResult::Ok } else {
                    log::error!("profiler_install_apk: {}", resp.message);
                    ProfilerResult::OperationFailed
                }
            }
            Err(e) => {
                log::error!("profiler_install_apk: {e:#}");
                ProfilerResult::OperationFailed
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Surface Names
// ---------------------------------------------------------------------------

/// Get SurfaceFlinger surface names matching a package.
///
/// Returns a newline-joined UTF-16 string.  Caller must free with `profiler_free_string`.
#[no_mangle]
pub extern "C" fn profiler_get_surface_names(
    serial: *const u16,
    package_name: *const u16,
    out: *mut *mut u16,
) -> ProfilerResult {
    if package_name.is_null() || out.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let package_str = unsafe { from_wide_ptr(package_name) };

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.get_surface_names(&package_str)) {
            Ok(names) => {
                let joined = names.join("\n");
                unsafe { *out = to_wide_ptr(&joined); }
                ProfilerResult::Ok
            }
            Err(e) => {
                log::error!("profiler_get_surface_names: {e:#}");
                unsafe { *out = ptr::null_mut(); }
                ProfilerResult::OperationFailed
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Charging Control
// ---------------------------------------------------------------------------

/// Enable or disable charging on the device.
#[no_mangle]
pub extern "C" fn profiler_set_charging(
    serial: *const u16,
    enable: bool,
) -> ProfilerResult {
    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.set_charging(enable)) {
            Ok(resp) => {
                if resp.success { ProfilerResult::Ok } else {
                    log::error!("profiler_set_charging: {}", resp.message);
                    ProfilerResult::OperationFailed
                }
            }
            Err(e) => {
                log::error!("profiler_set_charging: {e:#}");
                ProfilerResult::OperationFailed
            }
        }
    })
}

// ---------------------------------------------------------------------------
// File Operations
// ---------------------------------------------------------------------------

/// Remove a file on the device.
#[no_mangle]
pub extern "C" fn profiler_remove_file(
    serial: *const u16,
    path: *const u16,
) -> ProfilerResult {
    if path.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let path_str = unsafe { from_wide_ptr(path) };

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.remove_file(&path_str)) {
            Ok(resp) => {
                if resp.success { ProfilerResult::Ok } else {
                    log::error!("profiler_remove_file: {}", resp.message);
                    ProfilerResult::OperationFailed
                }
            }
            Err(e) => {
                log::error!("profiler_remove_file: {e:#}");
                ProfilerResult::OperationFailed
            }
        }
    })
}

/// Create a tar.gz archive on the device.
#[no_mangle]
pub extern "C" fn profiler_create_archive(
    serial: *const u16,
    working_directory: *const u16,
    target: *const u16,
    output_path: *const u16,
) -> ProfilerResult {
    if working_directory.is_null() || target.is_null() || output_path.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let dir_str = unsafe { from_wide_ptr(working_directory) };
    let target_str = unsafe { from_wide_ptr(target) };
    let output_str = unsafe { from_wide_ptr(output_path) };

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.create_archive(&dir_str, &target_str, &output_str)) {
            Ok(resp) => {
                if resp.success { ProfilerResult::Ok } else {
                    log::error!("profiler_create_archive: {}", resp.message);
                    ProfilerResult::OperationFailed
                }
            }
            Err(e) => {
                log::error!("profiler_create_archive: {e:#}");
                ProfilerResult::OperationFailed
            }
        }
    })
}

/// Extract a tar.gz archive on the device.
#[no_mangle]
pub extern "C" fn profiler_extract_archive(
    serial: *const u16,
    working_directory: *const u16,
    archive_path: *const u16,
) -> ProfilerResult {
    if working_directory.is_null() || archive_path.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let dir_str = unsafe { from_wide_ptr(working_directory) };
    let archive_str = unsafe { from_wide_ptr(archive_path) };

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.extract_archive(&dir_str, &archive_str)) {
            Ok(resp) => {
                if resp.success { ProfilerResult::Ok } else {
                    log::error!("profiler_extract_archive: {}", resp.message);
                    ProfilerResult::OperationFailed
                }
            }
            Err(e) => {
                log::error!("profiler_extract_archive: {e:#}");
                ProfilerResult::OperationFailed
            }
        }
    })
}

/// Change file permissions on the device.
#[no_mangle]
pub extern "C" fn profiler_chmod(
    serial: *const u16,
    path: *const u16,
    mode: *const u16,
    recursive: bool,
) -> ProfilerResult {
    if path.is_null() || mode.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let path_str = unsafe { from_wide_ptr(path) };
    let mode_str = unsafe { from_wide_ptr(mode) };

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.chmod(&path_str, &mode_str, recursive)) {
            Ok(resp) => {
                if resp.success { ProfilerResult::Ok } else {
                    log::error!("profiler_chmod: {}", resp.message);
                    ProfilerResult::OperationFailed
                }
            }
            Err(e) => {
                log::error!("profiler_chmod: {e:#}");
                ProfilerResult::OperationFailed
            }
        }
    })
}

/// Change file ownership on the device.
#[no_mangle]
pub extern "C" fn profiler_chown(
    serial: *const u16,
    path: *const u16,
    uid: i32,
    gid: i32,
    recursive: bool,
) -> ProfilerResult {
    if path.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let path_str = unsafe { from_wide_ptr(path) };

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.chown(&path_str, uid, gid, recursive)) {
            Ok(resp) => {
                if resp.success { ProfilerResult::Ok } else {
                    log::error!("profiler_chown: {}", resp.message);
                    ProfilerResult::OperationFailed
                }
            }
            Err(e) => {
                log::error!("profiler_chown: {e:#}");
                ProfilerResult::OperationFailed
            }
        }
    })
}

/// Get the UID owner of a file on the device.
#[no_mangle]
pub extern "C" fn profiler_get_file_owner(
    serial: *const u16,
    path: *const u16,
    uid_out: *mut i32,
) -> ProfilerResult {
    if path.is_null() || uid_out.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let path_str = unsafe { from_wide_ptr(path) };

    with_connection!(serial, |rt, entry| {
        match rt.block_on(entry.client.get_file_owner(&path_str)) {
            Ok(uid) => {
                unsafe { *uid_out = uid; }
                ProfilerResult::Ok
            }
            Err(e) => {
                log::error!("profiler_get_file_owner: {e:#}");
                ProfilerResult::OperationFailed
            }
        }
    })
}

// ---------------------------------------------------------------------------
// Cache Report (CR) streaming
// ---------------------------------------------------------------------------

/// Start a Cache Report stream.
///
/// On success, `handle_out` receives an opaque handle id.  Use
/// `profiler_poll_cr` to read data and `profiler_stop_cr` to stop.
#[no_mangle]
pub extern "C" fn profiler_start_cr(
    serial: *const u16,
    interval_secs: f64,
    cpus: *const i32,
    cpus_count: usize,
    exclude_kernel: bool,
    diff_kernel: bool,
    full_mode: bool,
    custom_events: *const u32,
    custom_events_count: usize,
    handle_out: *mut u64,
) -> ProfilerResult {
    if serial.is_null() || handle_out.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let serial_str = unsafe { from_wide_ptr(serial) };
    let cpus_slice: &[i32] = if cpus.is_null() || cpus_count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(cpus, cpus_count) }
    };
    let events_slice: &[u32] = if custom_events.is_null() || custom_events_count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(custom_events, custom_events_count) }
    };
    let rt = crate::runtime();

    let mut conns = crate::connections().lock();
    let entry = match conns.get_mut(&serial_str) {
        Some(e) => e,
        None => return ProfilerResult::DeviceNotFound,
    };

    match rt.block_on(entry.client.start_cr_stream(interval_secs, cpus_slice, exclude_kernel, diff_kernel, full_mode, events_slice)) {
        Ok(stream) => {
            let handle_id = crate::next_cr_handle_id();
            let handle = grpc::streaming::CrStreamHandle::start(rt, stream, 512);
            crate::cr_handles().lock().insert(handle_id, handle);
            unsafe { *handle_out = handle_id; }
            ProfilerResult::Ok
        }
        Err(e) => {
            log::error!("profiler_start_cr: {e:#}");
            ProfilerResult::OperationFailed
        }
    }
}

/// Convert a slice of proto CrCpuMetrics into a heap-allocated FFI array.
/// Returns (ptr, count). Caller must free via Vec::from_raw_parts.
fn proto_cpus_to_ffi(
    cpus: &[crate::proto::CrCpuMetrics],
) -> (*mut ProfilerCrCpuMetrics, usize) {
    let count = cpus.len();
    if count == 0 {
        return (ptr::null_mut(), 0);
    }
    let mut ffi_cpus: Vec<ProfilerCrCpuMetrics> = cpus
        .iter()
        .map(|c| ProfilerCrCpuMetrics {
            cpu_num: c.cpu_num,
            cpu_freq_mhz: c.cpu_freq_mhz,
            cpu_usage_pct: c.cpu_usage_pct,
            mips: c.mips,
            mcps: c.mcps,
            cpi: c.cpi,
            execution_mcps: c.execution_mcps,
            stall_ratio_pct: c.stall_ratio_pct,
            be_stall_ratio_pct: c.be_stall_ratio_pct,
            fe_stall_ratio_pct: c.fe_stall_ratio_pct,
            stall_mcps: c.stall_mcps,
            l1d_refill_ratio_pct: c.l1d_refill_ratio_pct,
            l2d_refill_ratio_pct: c.l2d_refill_ratio_pct,
            l3d_refill_ratio_pct: c.l3d_refill_ratio_pct,
            llc_read_hit_ratio_pct: c.llc_read_hit_ratio_pct,
            l1d_mpki: c.l1d_mpki,
            l2d_mpki: c.l2d_mpki,
            l3d_mpki: c.l3d_mpki,
            branch_mpki: c.branch_mpki,
            dtlb_mpki: c.dtlb_mpki,
            itlb_mpki: c.itlb_mpki,
            branch_miss_rate_pct: c.branch_miss_rate_pct,
        })
        .collect();
    let p = ffi_cpus.as_mut_ptr();
    std::mem::forget(ffi_cpus);
    (p, count)
}

/// Non-blocking poll for the next CR data point.
///
/// Returns `true` if data was available and `out` was populated.
/// Returns `false` if no data is available yet (caller should try again).
#[no_mangle]
pub extern "C" fn profiler_poll_cr(handle: u64, out: *mut ProfilerCrData) -> bool {
    if out.is_null() {
        return false;
    }

    let handles = crate::cr_handles().lock();
    let h = match handles.get(&handle) {
        Some(h) => h,
        None => return false,
    };

    match h.poll() {
        Some(dp) => {
            let (cpus_ptr, cpus_count) = proto_cpus_to_ffi(&dp.cpus);
            let (kernel_cpus_ptr, kernel_cpus_count) = proto_cpus_to_ffi(&dp.kernel_cpus);
            let (include_cpus_ptr, include_cpus_count) = proto_cpus_to_ffi(&dp.include_cpus);

            unsafe {
                (*out).timestamp_ms = dp.timestamp_ms;
                (*out).cpus = cpus_ptr;
                (*out).cpus_count = cpus_count;
                (*out).kernel_cpus = kernel_cpus_ptr;
                (*out).kernel_cpus_count = kernel_cpus_count;
                (*out).include_cpus = include_cpus_ptr;
                (*out).include_cpus_count = include_cpus_count;
            }
            true
        }
        None => false,
    }
}

/// Stop a CR stream and release the handle.
#[no_mangle]
pub extern "C" fn profiler_stop_cr(handle: u64) -> ProfilerResult {
    match crate::cr_handles().lock().remove(&handle) {
        Some(h) => {
            h.cancel();
            ProfilerResult::Ok
        }
        None => ProfilerResult::InvalidParameter,
    }
}

/// Free the dynamic arrays inside a `ProfilerCrData`.
#[no_mangle]
pub extern "C" fn profiler_free_cr_data(data: *mut ProfilerCrData) {
    if data.is_null() {
        return;
    }
    unsafe {
        let cpus_ptr = (*data).cpus;
        let cpus_count = (*data).cpus_count;
        if !cpus_ptr.is_null() && cpus_count > 0 {
            drop(Vec::from_raw_parts(cpus_ptr, cpus_count, cpus_count));
        }
        (*data).cpus = ptr::null_mut();
        (*data).cpus_count = 0;

        let kernel_cpus_ptr = (*data).kernel_cpus;
        let kernel_cpus_count = (*data).kernel_cpus_count;
        if !kernel_cpus_ptr.is_null() && kernel_cpus_count > 0 {
            drop(Vec::from_raw_parts(kernel_cpus_ptr, kernel_cpus_count, kernel_cpus_count));
        }
        (*data).kernel_cpus = ptr::null_mut();
        (*data).kernel_cpus_count = 0;

        let include_cpus_ptr = (*data).include_cpus;
        let include_cpus_count = (*data).include_cpus_count;
        if !include_cpus_ptr.is_null() && include_cpus_count > 0 {
            drop(Vec::from_raw_parts(include_cpus_ptr, include_cpus_count, include_cpus_count));
        }
        (*data).include_cpus = ptr::null_mut();
        (*data).include_cpus_count = 0;
    }
}

// ---------------------------------------------------------------------------
// Thread Cache (TC) streaming
// ---------------------------------------------------------------------------

/// Start a Thread Cache stream.
///
/// On success, `handle_out` receives an opaque handle id.  Use
/// `profiler_poll_tc` to read data and `profiler_stop_tc` to stop.
#[no_mangle]
pub extern "C" fn profiler_start_tc(
    serial: *const u16,
    pid: i32,
    interval_secs: f64,
    exclude_kernel: bool,
    diff_kernel: bool,
    top_threads_count: i32,
    handle_out: *mut u64,
) -> ProfilerResult {
    if serial.is_null() || handle_out.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let serial_str = unsafe { from_wide_ptr(serial) };
    let rt = crate::runtime();

    let mut conns = crate::connections().lock();
    let entry = match conns.get_mut(&serial_str) {
        Some(e) => e,
        None => return ProfilerResult::DeviceNotFound,
    };

    match rt.block_on(entry.client.start_tc_stream(pid, interval_secs, exclude_kernel, diff_kernel, top_threads_count)) {
        Ok(stream) => {
            let handle_id = crate::next_tc_handle_id();
            let handle = grpc::streaming::TcStreamHandle::start(rt, stream, 512);
            crate::tc_handles().lock().insert(handle_id, handle);
            unsafe { *handle_out = handle_id; }
            ProfilerResult::Ok
        }
        Err(e) => {
            log::error!("profiler_start_tc: {e:#}");
            ProfilerResult::OperationFailed
        }
    }
}

/// Convert a slice of proto TcThreadMetrics into an FFI array.
fn proto_tc_threads_to_ffi(
    threads: &[crate::proto::TcThreadMetrics],
) -> (*mut ProfilerTcThreadMetrics, usize) {
    let count = threads.len();
    if count == 0 {
        return (ptr::null_mut(), 0);
    }
    let mut ffi: Vec<ProfilerTcThreadMetrics> = threads
        .iter()
        .map(|t| ProfilerTcThreadMetrics {
            thread_id: t.thread_id,
            thread_name: to_wide_ptr(&t.thread_name),
            mips: t.mips,
            mcps: t.mcps,
            cpi: t.cpi,
            cpu_usage_pct: t.cpu_usage_pct,
            l1d_refill_ratio_pct: t.l1d_refill_ratio_pct,
            l1i_refill_ratio_pct: t.l1i_refill_ratio_pct,
            l2d_refill_ratio_pct: t.l2d_refill_ratio_pct,
            l3d_refill_ratio_pct: t.l3d_refill_ratio_pct,
            stall_ratio_pct: t.stall_ratio_pct,
            be_stall_ratio_pct: t.be_stall_ratio_pct,
            fe_stall_ratio_pct: t.fe_stall_ratio_pct,
            stall_mcps: t.stall_mcps,
            be_stall_mcps: t.be_stall_mcps,
            fe_stall_mcps: t.fe_stall_mcps,
            memory_instruction_pct: t.memory_instruction_pct,
        })
        .collect();
    let p = ffi.as_mut_ptr();
    std::mem::forget(ffi);
    (p, count)
}

/// Non-blocking poll for the next TC data point.
///
/// Returns `true` if data was available and `out` was populated.
/// Returns `false` if no data is available yet (caller should try again).
#[no_mangle]
pub extern "C" fn profiler_poll_tc(handle: u64, out: *mut ProfilerTcData) -> bool {
    if out.is_null() {
        return false;
    }

    let handles = crate::tc_handles().lock();
    let h = match handles.get(&handle) {
        Some(h) => h,
        None => return false,
    };

    match h.poll() {
        Some(dp) => {
            let (threads_ptr, threads_count) = proto_tc_threads_to_ffi(&dp.threads);
            let (kernel_ptr, kernel_count) = proto_tc_threads_to_ffi(&dp.kernel_threads);

            unsafe {
                (*out).timestamp_ms = dp.timestamp_ms;
                (*out).threads = threads_ptr;
                (*out).threads_count = threads_count;
                (*out).kernel_threads = kernel_ptr;
                (*out).kernel_threads_count = kernel_count;
            }
            true
        }
        None => false,
    }
}

/// Stop a TC stream and release the handle.
#[no_mangle]
pub extern "C" fn profiler_stop_tc(handle: u64) -> ProfilerResult {
    match crate::tc_handles().lock().remove(&handle) {
        Some(h) => {
            h.cancel();
            ProfilerResult::Ok
        }
        None => ProfilerResult::InvalidParameter,
    }
}

/// Free a TC thread metrics array (pointer + count).
unsafe fn free_tc_thread_array(ptr: *mut ProfilerTcThreadMetrics, count: usize) {
    if !ptr.is_null() && count > 0 {
        for i in 0..count {
            let thread = &mut *ptr.add(i);
            if !thread.thread_name.is_null() {
                free_wide_ptr(thread.thread_name);
                thread.thread_name = ptr::null_mut();
            }
        }
        drop(Vec::from_raw_parts(ptr, count, count));
    }
}

/// Free the dynamic arrays inside a `ProfilerTcData`.
#[no_mangle]
pub extern "C" fn profiler_free_tc_data(data: *mut ProfilerTcData) {
    if data.is_null() {
        return;
    }
    unsafe {
        free_tc_thread_array((*data).threads, (*data).threads_count);
        (*data).threads = ptr::null_mut();
        (*data).threads_count = 0;

        free_tc_thread_array((*data).kernel_threads, (*data).kernel_threads_count);
        (*data).kernel_threads = ptr::null_mut();
        (*data).kernel_threads_count = 0;
    }
}

// ---------------------------------------------------------------------------
// CML (Cache/Memory Latency) streaming
// ---------------------------------------------------------------------------

/// Start a CML benchmark stream.
///
/// On success, `handle_out` receives an opaque handle id.  Use
/// `profiler_poll_cml` to read data and `profiler_stop_cml` to stop.
#[no_mangle]
pub extern "C" fn profiler_start_cml(
    serial: *const u16,
    cpus: *const i32,
    cpus_count: usize,
    max_footprint_kb: u32,
    min_footprint_kb: u32,
    handle_out: *mut u64,
) -> ProfilerResult {
    if serial.is_null() || handle_out.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let serial_str = unsafe { from_wide_ptr(serial) };
    let cpus_slice: &[i32] = if cpus.is_null() || cpus_count == 0 {
        &[]
    } else {
        unsafe { std::slice::from_raw_parts(cpus, cpus_count) }
    };
    let rt = crate::runtime();

    let mut conns = crate::connections().lock();
    let entry = match conns.get_mut(&serial_str) {
        Some(e) => e,
        None => return ProfilerResult::DeviceNotFound,
    };

    match rt.block_on(entry.client.start_cml_stream(cpus_slice, max_footprint_kb, min_footprint_kb)) {
        Ok(stream) => {
            let handle_id = crate::next_cml_handle_id();
            let handle = grpc::streaming::CmlStreamHandle::start(rt, stream, 64);
            crate::cml_handles().lock().insert(handle_id, handle);
            unsafe { *handle_out = handle_id; }
            ProfilerResult::Ok
        }
        Err(e) => {
            let msg = format!("profiler_start_cml: {e:#}");
            log::error!("{msg}");
            crate::set_last_error(&msg);
            ProfilerResult::OperationFailed
        }
    }
}

/// Non-blocking poll for the next CML data point.
///
/// Returns `true` if data was available and `out` was populated.
/// Returns `false` if no data is available yet (caller should try again).
/// The `is_finished` field in `out` indicates whether the benchmark has completed.
#[no_mangle]
pub extern "C" fn profiler_poll_cml(handle: u64, out: *mut ProfilerCmlData) -> bool {
    if out.is_null() {
        return false;
    }

    let handles = crate::cml_handles().lock();
    let h = match handles.get(&handle) {
        Some(h) => h,
        None => return false,
    };

    let finished = h.is_finished();

    match h.poll() {
        Some(dp) => {
            let cpus_count = dp.cpus.len();
            let cpus_ptr = if cpus_count > 0 {
                let mut ffi_cpus: Vec<ProfilerCmlCpuLatency> = dp
                    .cpus
                    .iter()
                    .map(|c| ProfilerCmlCpuLatency {
                        cpu_id: c.cpu_id,
                        latency_ns: c.latency_ns,
                        error: c.error,
                    })
                    .collect();
                let p = ffi_cpus.as_mut_ptr();
                std::mem::forget(ffi_cpus);
                p
            } else {
                ptr::null_mut()
            };

            unsafe {
                (*out).footprint_kb = dp.footprint_kb;
                (*out).cpus = cpus_ptr;
                (*out).cpus_count = cpus_count;
                (*out).is_finished = finished;
            }
            true
        }
        None => {
            // No data, but still report finished status
            unsafe {
                (*out).footprint_kb = 0;
                (*out).cpus = ptr::null_mut();
                (*out).cpus_count = 0;
                (*out).is_finished = finished;
            }
            false
        }
    }
}

/// Stop a CML stream and release the handle.
#[no_mangle]
pub extern "C" fn profiler_stop_cml(handle: u64) -> ProfilerResult {
    match crate::cml_handles().lock().remove(&handle) {
        Some(h) => {
            h.cancel();
            ProfilerResult::Ok
        }
        None => ProfilerResult::InvalidParameter,
    }
}

/// Free the dynamic arrays inside a `ProfilerCmlData`.
#[no_mangle]
pub extern "C" fn profiler_free_cml_data(data: *mut ProfilerCmlData) {
    if data.is_null() {
        return;
    }
    unsafe {
        let cpus_ptr = (*data).cpus;
        let cpus_count = (*data).cpus_count;
        if !cpus_ptr.is_null() && cpus_count > 0 {
            drop(Vec::from_raw_parts(cpus_ptr, cpus_count, cpus_count));
        }
        (*data).cpus = ptr::null_mut();
        (*data).cpus_count = 0;
    }
}

/// Check whether a CML stream has finished (benchmark complete).
#[no_mangle]
pub extern "C" fn profiler_cml_is_finished(handle: u64) -> bool {
    let handles = crate::cml_handles().lock();
    match handles.get(&handle) {
        Some(h) => h.is_finished(),
        None => true,
    }
}
