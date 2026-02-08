use std::ffi::c_void;

/// Result codes returned by FFI functions.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProfilerResult {
    Ok = 0,
    DeviceNotFound = 1,
    DaemonNotRunning = 2,
    ConnectionFailed = 3,
    OperationFailed = 4,
    InvalidParameter = 5,
    Unknown = 99,
}

/// A single device discovered by `adb devices`.
#[repr(C)]
pub struct ProfilerDevice {
    /// Null-terminated UTF-16 device serial.
    pub serial: *mut u16,
    /// Null-terminated UTF-16 model name.
    pub model: *mut u16,
    /// Null-terminated UTF-16 state (e.g. "device", "offline").
    pub state: *mut u16,
}

/// A list of devices returned to C#.
#[repr(C)]
pub struct ProfilerDeviceList {
    /// Pointer to an array of `ProfilerDevice`.
    pub devices: *mut ProfilerDevice,
    /// Number of devices in the array.
    pub count: usize,
}

/// Information about the foreground (top) app on the device.
#[repr(C)]
pub struct ProfilerTopApp {
    /// Null-terminated UTF-16 package name.
    pub package_name: *mut u16,
    /// Null-terminated UTF-16 activity class name.
    pub activity: *mut u16,
    /// PID of the top app.
    pub pid: i32,
}

/// A single real-time benchmark data point.
#[repr(C)]
pub struct ProfilerRtbData {
    pub timestamp_ms: u64,
    pub fps: f64,
    pub power_mw: f64,
    pub battery_temp_c: f64,
    pub gpu_freq_mhz: f64,
    pub gpu_loading_pct: f64,
    pub bcpu_freq_mhz: f64,
    pub mcpu_freq_mhz: f64,
    pub lcpu_freq_mhz: f64,
    /// Per-core CPU frequencies.
    pub cpu_freqs_mhz: *mut f64,
    pub cpu_freqs_count: usize,
    /// Per-core CPU usages.
    pub cpu_usages_pct: *mut f64,
    pub cpu_usages_count: usize,
    pub total_mips: f64,
    pub dsu_freq_mhz: f64,
    pub dram_freq_mbps: f64,
    pub vcore_v: f64,
    pub wss_kb: u64,
}

/// Status of a Perfetto trace session.
#[repr(C)]
pub struct ProfilerPerfettoStatus {
    /// State enum: 0=IDLE, 1=RECORDING, 3=DONE, 4=ERROR
    pub state: i32,
    /// Progress percentage [0.0 .. 100.0].
    pub progress_pct: f64,
    /// Null-terminated UTF-16 output file path on device.
    pub output_path: *mut u16,
}

/// Health check result from the daemon.
#[repr(C)]
pub struct ProfilerHealthInfo {
    /// Null-terminated UTF-16 daemon version string.
    pub version: *mut u16,
    /// Null-terminated UTF-16 status string (e.g. "ok").
    pub status: *mut u16,
}

/// Progress callback type for file pull/push operations.
pub type ProgressCallback =
    extern "C" fn(bytes_done: u64, total_bytes: u64, user_data: *mut c_void);

// ---------------------------------------------------------------------------
// New FFI types for gRPC migration
// ---------------------------------------------------------------------------

/// Package info for a single installed app.
#[repr(C)]
pub struct ProfilerPackageInfo {
    pub package_name: *mut u16,
    pub apk_path: *mut u16,
    pub version_name: *mut u16,
    pub version_code: i32,
    pub pid: i32,
}

/// A list of packages returned to C#.
#[repr(C)]
pub struct ProfilerPackageList {
    pub packages: *mut ProfilerPackageInfo,
    pub count: usize,
}

/// Shell command result.
#[repr(C)]
pub struct ProfilerShellResult {
    pub exit_code: i32,
    pub stdout: *mut u16,
    pub stderr: *mut u16,
}
