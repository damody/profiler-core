pub mod ffi;
pub mod adb;
pub mod grpc;

use std::collections::HashMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Once, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use tokio::runtime::Runtime;
#[cfg(target_os = "windows")]
use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_POINTERS,
};

/// Last error message from FFI operations (thread-local would be ideal but
/// OnceLock<Mutex<>> is consistent with the rest of our global state).
static LAST_ERROR: OnceLock<Mutex<String>> = OnceLock::new();
static PANIC_HOOK_INIT: Once = Once::new();
static SEH_HOOK_INIT: Once = Once::new();
static CRASH_LOG_RESET_INIT: Once = Once::new();

use crate::grpc::client::ProfilerClient;
use crate::grpc::streaming::{RtbStreamHandle, CrStreamHandle, TcStreamHandle, CmlStreamHandle, GcStreamHandle};

/// Global tokio runtime, lazily initialized via `init_runtime()`.
static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// Active gRPC connections keyed by device serial.
static CONNECTIONS: OnceLock<Mutex<HashMap<String, ConnectionEntry>>> = OnceLock::new();

/// Active RTB stream handles keyed by handle id.
static RTB_HANDLES: OnceLock<Mutex<HashMap<u64, RtbStreamHandle>>> = OnceLock::new();

/// Counter for generating unique RTB handle ids.
static NEXT_RTB_HANDLE: OnceLock<Mutex<u64>> = OnceLock::new();

/// Active CR (Cache Report) stream handles keyed by handle id.
static CR_HANDLES: OnceLock<Mutex<HashMap<u64, CrStreamHandle>>> = OnceLock::new();

/// Counter for generating unique CR handle ids.
static NEXT_CR_HANDLE: OnceLock<Mutex<u64>> = OnceLock::new();

/// Active TC (Thread Cache) stream handles keyed by handle id.
static TC_HANDLES: OnceLock<Mutex<HashMap<u64, TcStreamHandle>>> = OnceLock::new();

/// Counter for generating unique TC handle ids.
static NEXT_TC_HANDLE: OnceLock<Mutex<u64>> = OnceLock::new();

/// Active CML (Cache/Memory Latency) stream handles keyed by handle id.
static CML_HANDLES: OnceLock<Mutex<HashMap<u64, CmlStreamHandle>>> = OnceLock::new();

/// Counter for generating unique CML handle ids.
static NEXT_CML_HANDLE: OnceLock<Mutex<u64>> = OnceLock::new();

/// Active GC (GPU Counters) stream handles keyed by handle id.
static GC_HANDLES: OnceLock<Mutex<HashMap<u64, GcStreamHandle>>> = OnceLock::new();

/// Counter for generating unique GC handle ids.
static NEXT_GC_HANDLE: OnceLock<Mutex<u64>> = OnceLock::new();

/// An active connection to a device's realtime_profile daemon.
pub struct ConnectionEntry {
    pub serial: String,
    pub client: ProfilerClient,
    pub port: u16,
}

/// Initialize the global tokio runtime. Returns true if it was freshly created.
pub fn init_runtime() -> bool {
    let created = RUNTIME
        .set(
            Runtime::new().expect("Failed to create tokio runtime"),
        )
        .is_ok();

    // Also initialise the other global maps
    let _ = CONNECTIONS.set(Mutex::new(HashMap::new()));
    let _ = RTB_HANDLES.set(Mutex::new(HashMap::new()));
    let _ = NEXT_RTB_HANDLE.set(Mutex::new(1));
    let _ = CR_HANDLES.set(Mutex::new(HashMap::new()));
    let _ = NEXT_CR_HANDLE.set(Mutex::new(1));
    let _ = TC_HANDLES.set(Mutex::new(HashMap::new()));
    let _ = NEXT_TC_HANDLE.set(Mutex::new(1));
    let _ = CML_HANDLES.set(Mutex::new(HashMap::new()));
    let _ = NEXT_CML_HANDLE.set(Mutex::new(1));
    let _ = GC_HANDLES.set(Mutex::new(HashMap::new()));
    let _ = NEXT_GC_HANDLE.set(Mutex::new(1));
    let _ = LAST_ERROR.set(Mutex::new(String::new()));

    if created {
        log::info!("profiler-core runtime initialized");
    }
    created
}

/// Shutdown the global runtime.
/// Because `OnceLock` does not expose `take()`, we drop all connection state
/// but cannot fully destroy the runtime. The process exit will clean it up.
pub fn shutdown_runtime() {
    // Drop all RTB handles (cancels streams)
    if let Some(handles) = RTB_HANDLES.get() {
        let mut map = handles.lock();
        for (_id, handle) in map.drain() {
            handle.cancel();
        }
    }

    // Drop all CR handles (cancels streams)
    if let Some(handles) = CR_HANDLES.get() {
        let mut map = handles.lock();
        for (_id, handle) in map.drain() {
            handle.cancel();
        }
    }

    // Drop all TC handles (cancels streams)
    if let Some(handles) = TC_HANDLES.get() {
        let mut map = handles.lock();
        for (_id, handle) in map.drain() {
            handle.cancel();
        }
    }

    // Drop all CML handles (cancels streams)
    if let Some(handles) = CML_HANDLES.get() {
        let mut map = handles.lock();
        for (_id, handle) in map.drain() {
            handle.cancel();
        }
    }

    // Drop all GC handles (cancels streams)
    if let Some(handles) = GC_HANDLES.get() {
        let mut map = handles.lock();
        for (_id, handle) in map.drain() {
            handle.cancel();
        }
    }

    // Shutdown all connected daemons via gRPC, then drop connections
    if let Some(conns) = CONNECTIONS.get() {
        let mut map = conns.lock();
        if let Some(rt) = RUNTIME.get() {
            for (_serial, entry) in map.iter_mut() {
                let _ = rt.block_on(entry.client.shutdown());
            }
        }
        map.clear();
    }

    log::info!("profiler-core shutdown complete");
}

/// Get a reference to the global runtime, panics if not initialised.
pub fn runtime() -> &'static Runtime {
    RUNTIME.get().expect("Runtime not initialized – call profiler_init() first")
}

/// Get the connections map.
pub fn connections() -> &'static Mutex<HashMap<String, ConnectionEntry>> {
    CONNECTIONS.get().expect("Runtime not initialized")
}

/// Get the RTB handles map.
pub fn rtb_handles() -> &'static Mutex<HashMap<u64, RtbStreamHandle>> {
    RTB_HANDLES.get().expect("Runtime not initialized")
}

/// Allocate a new unique RTB handle id.
pub fn next_rtb_handle_id() -> u64 {
    let counter = NEXT_RTB_HANDLE.get().expect("Runtime not initialized");
    let mut val = counter.lock();
    let id = *val;
    *val += 1;
    id
}

/// Get the CR handles map.
pub fn cr_handles() -> &'static Mutex<HashMap<u64, CrStreamHandle>> {
    CR_HANDLES.get().expect("Runtime not initialized")
}

/// Allocate a new unique CR handle id.
pub fn next_cr_handle_id() -> u64 {
    let counter = NEXT_CR_HANDLE.get().expect("Runtime not initialized");
    let mut val = counter.lock();
    let id = *val;
    *val += 1;
    id
}

/// Get the TC handles map.
pub fn tc_handles() -> &'static Mutex<HashMap<u64, TcStreamHandle>> {
    TC_HANDLES.get().expect("Runtime not initialized")
}

/// Allocate a new unique TC handle id.
pub fn next_tc_handle_id() -> u64 {
    let counter = NEXT_TC_HANDLE.get().expect("Runtime not initialized");
    let mut val = counter.lock();
    let id = *val;
    *val += 1;
    id
}

/// Get the CML handles map.
pub fn cml_handles() -> &'static Mutex<HashMap<u64, CmlStreamHandle>> {
    CML_HANDLES.get().expect("Runtime not initialized")
}

/// Allocate a new unique CML handle id.
pub fn next_cml_handle_id() -> u64 {
    let counter = NEXT_CML_HANDLE.get().expect("Runtime not initialized");
    let mut val = counter.lock();
    let id = *val;
    *val += 1;
    id
}

/// Get the GC handles map.
pub fn gc_handles() -> &'static Mutex<HashMap<u64, GcStreamHandle>> {
    GC_HANDLES.get().expect("Runtime not initialized")
}

/// Allocate a new unique GC handle id.
pub fn next_gc_handle_id() -> u64 {
    let counter = NEXT_GC_HANDLE.get().expect("Runtime not initialized");
    let mut val = counter.lock();
    let id = *val;
    *val += 1;
    id
}

fn crash_log_path() -> PathBuf {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(parent) = exe.parent() {
            return parent.join("crash.log");
        }
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join("crash.log")
}

fn append_crash_log_internal(kind: &str, detail: &str) {
    let path = crash_log_path();
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(file, "[{ts}] {kind}");
        let _ = writeln!(file, "{detail}");
        let _ = writeln!(file);
    }
}

fn append_crash_log(kind: &str, detail: &str) {
    append_crash_log_internal(kind, detail);
}

fn reset_crash_log_for_current_run() {
    CRASH_LOG_RESET_INIT.call_once(|| {
        let path = crash_log_path();
        let _ = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(path);
    });
}

#[cfg(target_os = "windows")]
unsafe extern "system" fn vectored_exception_handler(
    exception_info: *mut EXCEPTION_POINTERS,
) -> i32 {
    if exception_info.is_null() {
        return 0;
    }
    let record = unsafe { (*exception_info).ExceptionRecord };
    if record.is_null() {
        return 0;
    }

    let code = unsafe { (*record).ExceptionCode as u32 };
    let should_log = matches!(
        code,
        0xC0000094 | // integer divide by zero
        0xC0000005 | // access violation
        0xC0000409 | // stack buffer overrun / fast fail
        0xC000001D | // illegal instruction
        0xC00000FD | // stack overflow
        0x80000003   // breakpoint
    );
    if !should_log {
        return 0;
    }
    let flags = unsafe { (*record).ExceptionFlags as u32 };
    let address = unsafe { (*record).ExceptionAddress as usize };
    let backtrace = std::backtrace::Backtrace::force_capture();
    let detail = format!(
        "code: 0x{code:08X}\nflags: 0x{flags:08X}\naddress: 0x{address:016X}\nbacktrace:\n{backtrace}"
    );
    append_crash_log("windows seh exception", &detail);

    0
}

#[cfg(target_os = "windows")]
fn install_windows_seh_hook() {
    SEH_HOOK_INIT.call_once(|| unsafe {
        let handle = AddVectoredExceptionHandler(1, Some(vectored_exception_handler));
        if handle.is_null() {
            append_crash_log_internal(
                "crash hook init",
                "AddVectoredExceptionHandler install failed",
            );
        } else {
            append_crash_log_internal(
                "crash hook init",
                "AddVectoredExceptionHandler installed",
            );
        }
    });
}

pub fn install_crash_hook() {
    reset_crash_log_for_current_run();

    PANIC_HOOK_INIT.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
                (*s).to_string()
            } else if let Some(s) = info.payload().downcast_ref::<String>() {
                s.clone()
            } else {
                "unknown panic payload".to_string()
            };
            let location = info
                .location()
                .map(|loc| format!("{}:{}:{}", loc.file(), loc.line(), loc.column()))
                .unwrap_or_else(|| "unknown location".to_string());
            let thread_name = std::thread::current()
                .name()
                .unwrap_or("unnamed")
                .to_string();
            let backtrace = std::backtrace::Backtrace::force_capture();
            let detail = format!(
                "thread: {thread_name}\nlocation: {location}\npayload: {payload}\nbacktrace:\n{backtrace}"
            );

            append_crash_log("rust panic", &detail);

            if let Some(err) = LAST_ERROR.get() {
                *err.lock() = format!("Rust panic at {location}: {payload}");
            }

            prev(info);
        }));
    });

    #[cfg(target_os = "windows")]
    install_windows_seh_hook();
}

/// Store an error message that can be retrieved by C# via `profiler_get_last_error`.
pub fn set_last_error(msg: impl Into<String>) {
    let msg = msg.into();
    if let Some(err) = LAST_ERROR.get() {
        *err.lock() = msg.clone();
    }
    append_crash_log("rust error", &msg);
}

/// Take the last error message (returns empty string if none).
pub fn take_last_error() -> String {
    LAST_ERROR
        .get()
        .map(|m| std::mem::take(&mut *m.lock()))
        .unwrap_or_default()
}

// Re-export the shared protobuf code from mprofiler-proto.
pub use mprofiler_proto::mprofiler as proto;
