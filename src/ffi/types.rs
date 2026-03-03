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
    pub fps_dequeue: f64,
    pub fps_queue: f64,
    pub fps_present_fence: f64,
    pub power_mw: f64,
    pub power_ma: f64,
    pub voltage_v: f64,
    pub board_temp_c: f64,
    pub battery_temp_c: f64,
    pub gpu_freq_mhz: f64,
    pub gpu_loading_pct: f64,
    pub bcpu_freq_mhz: f64,
    pub bcpu_usage_pct: f64,
    pub mcpu_freq_mhz: f64,
    pub mcpu_usage_pct: f64,
    pub lcpu_freq_mhz: f64,
    pub lcpu_usage_pct: f64,
    /// Per-core CPU frequencies.
    pub cpu_freqs_mhz: *mut f64,
    pub cpu_freqs_count: usize,
    /// Per-core CPU usages.
    pub cpu_usages_pct: *mut f64,
    pub cpu_usages_count: usize,
    pub total_mips: f64,
    pub game_mips: f64,
    pub logical_mips: f64,
    pub render_mips: f64,
    pub rhi_mips: f64,
    pub dsu_freq_mhz: f64,
    pub dram_freq_mhz: f64,
    pub vcore_v: f64,
    pub wss_kb: u64,
    pub pss_kb: u64,
    pub top_app_rss_kb: u64,
    /// Frame times collected during this interval.
    pub frame_times_ms: *mut f32,
    pub frame_times_count: usize,
    pub cpu_time_ms: f64,
    pub gpu_time_ms: f64,
    pub power_avg_mw: f64,
    /// Nullable UTF-16 process/surface name.
    pub process_name: *mut u16,
}

/// RTB stream metric selection options (passed from C#).
#[repr(C)]
pub struct ProfilerRtbOptions {
    pub enable_cpu_loading: bool,
    pub enable_cpu_freq: bool,
    pub enable_fps_dequeue: bool,
    pub enable_fps_queue: bool,
    pub enable_fps_present_fence: bool,
    pub enable_gpu: bool,
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

/// Temperature reading from the device.
#[repr(C)]
pub struct ProfilerTemperature {
    pub battery_temp_c: f64,
    pub board_temp_c: f64,
    pub battery_level_pct: u32,
}

// ---------------------------------------------------------------------------
// RTB Summary types
// ---------------------------------------------------------------------------

/// A single thread snapshot from RTB summary.
#[repr(C)]
pub struct ProfilerThreadSnapshot {
    pub tid: i32,
    pub tgid: i32,
    pub name: *mut u16,
    pub loading_pct: f64,
    pub c0_pct: f64,
    pub c1_pct: f64,
    pub c2_pct: f64,
    pub runnable_pct: f64,
    pub mips: f64,
    pub mcps: f64,
    pub cpi: f64,
}

/// A single frequency bucket in a distribution.
#[repr(C)]
pub struct ProfilerFreqBucket {
    pub freq_mhz: u64,
    pub count: u32,
    pub percentage: f64,
}

/// A frequency distribution for a component (e.g. "C0", "GPU", "DSU").
#[repr(C)]
pub struct ProfilerFreqDistribution {
    pub component: *mut u16,
    pub buckets: *mut ProfilerFreqBucket,
    pub buckets_count: usize,
}

// ---------------------------------------------------------------------------
// Cache Report (CR) types
// ---------------------------------------------------------------------------

/// Per-CPU cache report metrics.
#[repr(C)]
pub struct ProfilerCrCpuMetrics {
    pub cpu_num: i32,
    pub cpu_freq_mhz: f64,
    pub cpu_usage_pct: f64,
    pub mips: f64,
    pub mcps: f64,
    pub cpi: f64,
    pub execution_mcps: f64,
    pub stall_ratio_pct: f64,
    pub be_stall_ratio_pct: f64,
    pub fe_stall_ratio_pct: f64,
    pub stall_mcps: f64,
    pub l1d_refill_ratio_pct: f64,
    pub l2d_refill_ratio_pct: f64,
    pub l3d_refill_ratio_pct: f64,
    pub llc_read_hit_ratio_pct: f64,
    pub l1d_mpki: f64,
    pub l2d_mpki: f64,
    pub l3d_mpki: f64,
    pub branch_mpki: f64,
    pub dtlb_mpki: f64,
    pub itlb_mpki: f64,
    pub branch_miss_rate_pct: f64,
}

/// A single cache report data point (one timestamp, multiple CPUs).
#[repr(C)]
pub struct ProfilerCrData {
    pub timestamp_ms: u64,
    pub cpus: *mut ProfilerCrCpuMetrics,
    pub cpus_count: usize,
    pub kernel_cpus: *mut ProfilerCrCpuMetrics,
    pub kernel_cpus_count: usize,
    pub include_cpus: *mut ProfilerCrCpuMetrics,
    pub include_cpus_count: usize,
}

/// RTB summary containing post-recording statistics.
#[repr(C)]
pub struct ProfilerRtbSummary {
    pub top_threads: *mut ProfilerThreadSnapshot,
    pub top_threads_count: usize,
    pub freq_distributions: *mut ProfilerFreqDistribution,
    pub freq_distributions_count: usize,
    pub logical_thread: ProfilerThreadSnapshot,
    pub render_thread: ProfilerThreadSnapshot,
    pub rhi_thread: ProfilerThreadSnapshot,
    pub start_temp: f64,
    pub end_temp: f64,
    pub frame_times_ms: *mut f32,
    pub frame_times_count: usize,
}

// ---------------------------------------------------------------------------
// Thread Cache (TC) types
// ---------------------------------------------------------------------------

/// Per-thread cache metrics.
#[repr(C)]
pub struct ProfilerTcThreadMetrics {
    pub thread_id: i32,
    pub thread_name: *mut u16,    // UTF-16 null-terminated
    pub mips: f64,
    pub mcps: f64,
    pub cpi: f64,
    pub cpu_usage_pct: f64,
    pub l1d_refill_ratio_pct: f64,
    pub l1i_refill_ratio_pct: f64,
    pub l2d_refill_ratio_pct: f64,
    pub l3d_refill_ratio_pct: f64,
    pub stall_ratio_pct: f64,
    pub be_stall_ratio_pct: f64,
    pub fe_stall_ratio_pct: f64,
    pub stall_mcps: f64,
    pub be_stall_mcps: f64,
    pub fe_stall_mcps: f64,
    pub memory_instruction_pct: f64,
}

/// A single thread cache data point (one timestamp, multiple threads).
#[repr(C)]
pub struct ProfilerTcData {
    pub timestamp_ms: u64,
    pub threads: *mut ProfilerTcThreadMetrics,
    pub threads_count: usize,
    pub kernel_threads: *mut ProfilerTcThreadMetrics,
    pub kernel_threads_count: usize,
}

// ---------------------------------------------------------------------------
// CML (Cache/Memory Latency) types
// ---------------------------------------------------------------------------

/// Per-CPU latency measurement from a CML benchmark step.
#[repr(C)]
pub struct ProfilerCmlCpuLatency {
    pub cpu_id: i32,
    // 4 bytes padding here (f64 alignment)
    pub latency_ns: f64,
    pub error: bool,
}

/// A single CML data point (one footprint size, multiple CPUs).
#[repr(C)]
pub struct ProfilerCmlData {
    pub footprint_kb: u32,
    // 4 bytes padding here (pointer alignment)
    pub cpus: *mut ProfilerCmlCpuLatency,
    pub cpus_count: usize,
    pub is_finished: bool,
}

// ---------------------------------------------------------------------------
// GPU Counters (GC) types
// ---------------------------------------------------------------------------

/// GPU information from HWCPipe discovery.
#[repr(C)]
pub struct ProfilerGpuInfo {
    pub device_number: u32,
    // 4 bytes padding (pointer alignment)
    pub gpu_family: *mut u16,    // UTF-16 null-terminated
    pub num_shader_cores: u32,
    pub num_exec_engines: u32,
    pub bus_width: u32,
    // 4 bytes padding (u64 alignment)
    pub product_id: u64,
}

/// GPU counter info from HWCPipe discovery.
#[repr(C)]
pub struct ProfilerGpuCounterInfo {
    pub counter_id: u32,
    // 4 bytes padding (pointer alignment)
    pub name: *mut u16,          // UTF-16 null-terminated
    pub units: *mut u16,         // UTF-16 null-terminated
}

/// Discovery result containing GPU info and available counters.
#[repr(C)]
pub struct ProfilerGcDiscoverResult {
    pub gpus: *mut ProfilerGpuInfo,
    pub gpus_count: usize,
    pub counters: *mut ProfilerGpuCounterInfo,
    pub counters_count: usize,
}

/// A single counter value in a GC data point.
#[repr(C)]
pub struct ProfilerGcCounterValue {
    pub counter_id: u32,
    // 4 bytes padding (f64 alignment)
    pub value: f64,
}

/// A single GPU Counters data point (one timestamp, multiple counter values).
#[repr(C)]
pub struct ProfilerGcData {
    pub timestamp_ms: u64,
    pub counters: *mut ProfilerGcCounterValue,
    pub counters_count: usize,
}
