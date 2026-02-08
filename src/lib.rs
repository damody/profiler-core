pub mod ffi;
pub mod adb;
pub mod grpc;

use std::collections::HashMap;
use std::sync::OnceLock;

use parking_lot::Mutex;
use tokio::runtime::Runtime;

use crate::grpc::client::ProfilerClient;
use crate::grpc::streaming::RtbStreamHandle;

/// Global tokio runtime, lazily initialized via `init_runtime()`.
static RUNTIME: OnceLock<Runtime> = OnceLock::new();

/// Active gRPC connections keyed by device serial.
static CONNECTIONS: OnceLock<Mutex<HashMap<String, ConnectionEntry>>> = OnceLock::new();

/// Active RTB stream handles keyed by handle id.
static RTB_HANDLES: OnceLock<Mutex<HashMap<u64, RtbStreamHandle>>> = OnceLock::new();

/// Counter for generating unique RTB handle ids.
static NEXT_RTB_HANDLE: OnceLock<Mutex<u64>> = OnceLock::new();

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

// Include the generated protobuf code.
pub mod proto {
    tonic::include_proto!("mprofiler");
}
