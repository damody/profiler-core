use anyhow::{Context, Result};
use tokio::time::{sleep, Duration};

use super::commands;

/// Root mode detected on the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RootMode {
    /// `adb root` succeeded — adbd is running as root.
    Adb,
    /// `su` is available (e.g. Magisk).
    Su,
    /// No root available — best effort.
    None,
}

/// Try to obtain root privileges and set SELinux to permissive.
///
/// Returns the [`RootMode`] that was achieved.
async fn ensure_root_and_permissive(serial: &str) -> RootMode {
    // --- Try `adb root` first ---
    match commands::root(serial).await {
        Ok(msg) => log::info!("adb root response: {msg}"),
        Err(e) => log::warn!("adb root failed: {e:#}"),
    }
    // adb root restarts adbd; wait for reconnection
    sleep(Duration::from_millis(2000)).await;

    if let Ok(true) = commands::is_root_shell(serial).await {
        log::info!("[{serial}] root mode: adb (adbd running as root)");
        // SELinux permissive
        let _ = commands::shell(serial, "setenforce 0").await;
        return RootMode::Adb;
    }

    // --- Fallback: try `su` (Magisk / KernelSU) ---
    match commands::shell_su(serial, "id").await {
        Ok(id_out) if id_out.contains("uid=0") => {
            log::info!("[{serial}] root mode: su");
            let _ = commands::shell_su(serial, "setenforce 0").await;
            return RootMode::Su;
        }
        Ok(id_out) => log::warn!("[{serial}] su returned non-root id: {id_out}"),
        Err(e) => log::warn!("[{serial}] su not available: {e:#}"),
    }

    log::warn!("[{serial}] root mode: none — daemon may not read sysfs nodes");
    RootMode::None
}

/// Deploy the realtime_profile binary to the device and start it as a
/// background gRPC daemon.
///
/// * `local_path` – local binary to push (empty string to skip push)
/// * `remote_path` – absolute path on device (e.g. `/data/local/tmp/realtime_profile`)
/// * `grpc_port` – port the daemon will listen on
pub async fn deploy_and_start(
    serial: &str,
    local_path: &str,
    remote_path: &str,
    grpc_port: u16,
) -> Result<()> {
    // 1. Push the binary (skip if local_path is empty)
    if !local_path.is_empty() {
        log::info!("Pushing realtime_profile to {serial}: {local_path} -> {remote_path}");
        commands::push(serial, local_path, remote_path)
            .await
            .context("Failed to push realtime_profile binary")?;
    }

    // 2. Make it executable
    commands::shell(serial, &format!("chmod +x {remote_path}"))
        .await
        .context("Failed to chmod realtime_profile")?;

    // 3. Kill any existing instance
    let _ = commands::shell(serial, "pkill -f realtime_profile").await;
    sleep(Duration::from_millis(500)).await;

    // 4. Ensure root + SELinux permissive
    let root_mode = ensure_root_and_permissive(serial).await;

    // 5. Start in background (with root if available)
    let daemon_args = format!("{remote_path} --daemon --grpc-port {grpc_port}");
    let start_cmd = match root_mode {
        RootMode::Adb => {
            // adbd is root — just run directly
            format!("nohup {daemon_args} > /dev/null 2>&1 &")
        }
        RootMode::Su => {
            // Use su -c to launch with root
            format!("nohup su -c '{daemon_args}' > /dev/null 2>&1 &")
        }
        RootMode::None => {
            // Best effort without root
            format!("nohup {daemon_args} > /dev/null 2>&1 &")
        }
    };

    commands::shell(serial, &start_cmd)
        .await
        .context("Failed to start realtime_profile daemon")?;

    // Give it a moment to boot
    sleep(Duration::from_millis(1000)).await;

    log::info!(
        "realtime_profile daemon started on {serial}, grpc_port {grpc_port}, root_mode: {root_mode:?}"
    );
    Ok(())
}

/// Check if the daemon is already running on the device.
pub async fn is_running(serial: &str) -> bool {
    match commands::shell(serial, "pidof realtime_profile").await {
        Ok(output) => !output.trim().is_empty(),
        Err(_) => false,
    }
}
