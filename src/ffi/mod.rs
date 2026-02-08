pub mod types;
pub mod utils;

use std::ffi::c_void;
use std::ptr;

use log;

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
    let _ = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("profiler_core=info"),
    )
    .try_init();
    crate::init_runtime();
    log::info!("profiler_init complete");
    ProfilerResult::Ok
}

/// Shut down the profiler runtime and release all resources.
#[no_mangle]
pub extern "C" fn profiler_shutdown() {
    crate::shutdown_runtime();
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
    if let Err(e) = rt.block_on(adb::commands::forward(&serial_str, port, port)) {
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

    match rt.block_on(entry.client.start_rtb_stream(pid, interval_secs)) {
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

            unsafe {
                (*out).timestamp_ms = dp.timestamp_ms;
                (*out).fps = dp.fps;
                (*out).power_mw = dp.power_mw;
                (*out).battery_temp_c = dp.battery_temp_c;
                (*out).gpu_freq_mhz = dp.gpu_freq_mhz;
                (*out).gpu_loading_pct = dp.gpu_loading_pct;
                (*out).bcpu_freq_mhz = dp.bcpu_freq_mhz;
                (*out).mcpu_freq_mhz = dp.mcpu_freq_mhz;
                (*out).lcpu_freq_mhz = dp.lcpu_freq_mhz;
                (*out).cpu_freqs_mhz = freqs_ptr;
                (*out).cpu_freqs_count = freqs_count;
                (*out).cpu_usages_pct = usages_ptr;
                (*out).cpu_usages_count = usages_count;
                (*out).total_mips = dp.total_mips;
                (*out).dsu_freq_mhz = dp.dsu_freq_mhz;
                (*out).dram_freq_mbps = dp.dram_freq_mbps;
                (*out).vcore_v = dp.vcore_v;
                (*out).wss_kb = dp.wss_kb;
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
    }
}

// ---------------------------------------------------------------------------
// Perfetto
// ---------------------------------------------------------------------------

/// Start a Perfetto trace session.
#[no_mangle]
pub extern "C" fn profiler_start_perfetto(
    serial: *const u16,
    pid: i32,
    mode: *const u16,
    duration_secs: i32,
) -> ProfilerResult {
    if serial.is_null() || mode.is_null() {
        return ProfilerResult::InvalidParameter;
    }
    let serial_str = unsafe { from_wide_ptr(serial) };
    let mode_str = unsafe { from_wide_ptr(mode) };
    let rt = crate::runtime();

    let mut conns = crate::connections().lock();
    let entry = match conns.get_mut(&serial_str) {
        Some(e) => e,
        None => return ProfilerResult::DeviceNotFound,
    };

    match rt.block_on(entry.client.start_perfetto(pid, &mode_str, duration_secs)) {
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
// Generic ADB shell
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
}

/// Free a wide string returned by `profiler_adb_shell`.
#[no_mangle]
pub extern "C" fn profiler_free_string(ptr: *mut u16) {
    unsafe { free_wide_ptr(ptr); }
}
