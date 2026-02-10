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

/// Push realtime_profile (and libc++_shared.so if present) to the device.
async fn push_binaries(serial: &str, local_path: &str, remote_path: &str) -> Result<()> {
    log::info!("Pushing realtime_profile to {serial}: {local_path} -> {remote_path}");
    commands::push(serial, local_path, remote_path)
        .await
        .context("Failed to push realtime_profile binary")?;

    // Also push libc++_shared.so if it exists next to the binary
    let local = std::path::Path::new(local_path);
    if let Some(dir) = local.parent() {
        let so_path = dir.join("libc++_shared.so");
        if so_path.exists() {
            let remote_dir = remote_path
                .rsplit_once('/')
                .map(|(d, _)| d)
                .unwrap_or("/data/local/tmp");
            let remote_so = format!("{remote_dir}/libc++_shared.so");
            log::info!(
                "Pushing libc++_shared.so to {serial}: {} -> {remote_so}",
                so_path.display()
            );
            commands::push(serial, &so_path.to_string_lossy(), &remote_so)
                .await
                .context("Failed to push libc++_shared.so")?;
        }
    }
    Ok(())
}

/// Assume the binary is already on-device; chmod, kill old instance, get root,
/// and start the daemon.
async fn start_daemon(serial: &str, remote_path: &str, grpc_port: u16) -> Result<()> {
    // Make it executable
    commands::shell(serial, &format!("chmod +x {remote_path}"))
        .await
        .context("Failed to chmod realtime_profile")?;

    // Kill any existing instance
    let _ = commands::shell(serial, "pkill -f realtime_profile").await;
    sleep(Duration::from_millis(500)).await;

    // Ensure root + SELinux permissive
    let root_mode = ensure_root_and_permissive(serial).await;

    // Start in background (with root if available)
    let remote_dir = remote_path
        .rsplit_once('/')
        .map(|(d, _)| d)
        .unwrap_or("/data/local/tmp");
    let ld_env = format!("LD_LIBRARY_PATH={remote_dir}");
    let daemon_args = format!("{remote_path} --daemon --grpc-port {grpc_port}");
    let start_cmd = match root_mode {
        RootMode::Adb => {
            // adbd is root — env var must precede nohup for shell to parse it
            format!("{ld_env} nohup {daemon_args} > /dev/null 2>&1 &")
        }
        RootMode::Su => {
            // su -c passes string to a shell, so env var assignment works inside quotes
            format!("nohup su -c '{ld_env} {daemon_args}' > /dev/null 2>&1 &")
        }
        RootMode::None => {
            format!("{ld_env} nohup {daemon_args} > /dev/null 2>&1 &")
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

/// Deploy the realtime_profile binary to the device and start it as a
/// background gRPC daemon.
///
/// Uses an optimistic strategy: first tries to start the daemon without
/// pushing (assuming a previous binary is still on-device). Only if that
/// fails does it push the binary and retry.
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
    // Phase 1: Optimistic start — try without pushing
    log::info!("[{serial}] 嘗試直接啟動 daemon（不 push）");
    if start_daemon(serial, remote_path, grpc_port).await.is_ok() && is_running(serial).await {
        log::info!("[{serial}] daemon 直接啟動成功，跳過 push");
        return Ok(());
    }
    log::info!("[{serial}] 直接啟動失敗，執行完整部署");

    // Phase 2: Full deploy — push binary then start
    if local_path.is_empty() {
        anyhow::bail!("Daemon 啟動失敗且未提供 local binary 路徑");
    }
    push_binaries(serial, local_path, remote_path).await?;
    start_daemon(serial, remote_path, grpc_port).await?;

    if !is_running(serial).await {
        anyhow::bail!("完整部署後 daemon 仍無法啟動");
    }
    log::info!("[{serial}] 完整部署後 daemon 啟動成功");
    Ok(())
}

/// Check if the daemon is already running on the device.
pub async fn is_running(serial: &str) -> bool {
    match commands::shell(serial, "pidof realtime_profile").await {
        Ok(output) => !output.trim().is_empty(),
        Err(_) => false,
    }
}
