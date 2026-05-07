use anyhow::{Context, Result};
use tokio::time::{sleep, Duration};

use super::commands;

/// Root mode detected on the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RootMode {
    /// `adb root` succeeded — adbd is running as root.
    Adb,
    /// `adb root` failed but `su` is available — commands need `su -c` wrapping.
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
    // adb root restarts adbd; poll until reconnected (up to 5s)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        sleep(Duration::from_millis(100)).await;
        if commands::shell(serial, "echo ok").await.is_ok() {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            log::warn!("adbd reconnect timed out after 5s");
            break;
        }
    }

    if let Ok(true) = commands::is_root_shell(serial).await {
        log::info!("[{serial}] root mode: adb (adbd running as root)");
        // SELinux permissive
        let _ = commands::shell(serial, "setenforce 0").await;
        return RootMode::Adb;
    }

    // --- Fallback: try `su` (e.g. Magisk on Qualcomm devices) ---
    log::info!("[{serial}] adb root 不可用，嘗試 su fallback...");
    if let Ok(id_output) = commands::shell(serial, "su -c id").await {
        if id_output.contains("uid=0") {
            log::info!("[{serial}] root mode: su (su -c id 成功)");
            let _ = commands::shell(serial, "su -c setenforce 0").await;
            return RootMode::Su;
        }
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
async fn start_daemon(
    serial: &str,
    remote_path: &str,
    grpc_port: u16,
    low_overhead: bool,
) -> Result<()> {
    // Make it executable
    commands::shell(serial, &format!("chmod +x {remote_path}"))
        .await
        .context("Failed to chmod realtime_profile")?;

    // Ensure root + SELinux permissive (before kill, so we know if su is needed)
    let root_mode = ensure_root_and_permissive(serial).await;

    // Kill any existing instance
    let _ = commands::shell(serial, "pkill -f realtime_profile").await;
    // If a root daemon is running and we're non-root shell, pkill may fail — try su
    if root_mode == RootMode::Su && is_running(serial).await {
        log::info!("[{serial}] pkill 未能殺掉 root daemon，使用 su -c pkill");
        let _ = commands::shell(serial, "su -c \"pkill -f realtime_profile\"").await;
    }
    // Poll until process is dead (up to 2s)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        sleep(Duration::from_millis(100)).await;
        if !is_running(serial).await {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            log::warn!("pkill wait timed out after 2s");
            break;
        }
    }

    // Start in background (with root if available)
    let remote_dir = remote_path
        .rsplit_once('/')
        .map(|(d, _)| d)
        .unwrap_or("/data/local/tmp");
    let ld_env = format!("LD_LIBRARY_PATH={remote_dir}");
    // 低負載模式預設開啟；關閉時明確使用 legacy mode 方便回滾驗證。
    let overhead_flag = if low_overhead {
        "--low-overhead --sync-only"
    } else {
        "--legacy-mode"
    };
    let daemon_args = format!("{remote_path} --daemon --grpc-port {grpc_port} {overhead_flag}");
    let start_cmd = match root_mode {
        RootMode::Adb | RootMode::None => {
            // adbd is root (Adb) or no root available (None) — run directly
            format!("{ld_env} nohup {daemon_args} > /dev/null 2>&1 &")
        }
        RootMode::Su => {
            // su available — wrap the entire command in su -c
            format!("su -c \"{ld_env} nohup {daemon_args} > /dev/null 2>&1 &\"")
        }
    };

    commands::shell(serial, &start_cmd)
        .await
        .context("Failed to start realtime_profile daemon")?;

    // Poll until daemon is running (up to 3s)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    loop {
        sleep(Duration::from_millis(100)).await;
        if is_running(serial).await {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            log::warn!("daemon boot wait timed out after 3s");
            break;
        }
    }

    log::info!(
        "realtime_profile daemon started on {serial}, grpc_port {grpc_port}, root_mode: {root_mode:?}"
    );
    Ok(())
}

/// Deploy the realtime_profile binary to the device and start it as a
/// background gRPC daemon.
///
/// When a local binary is provided, always push it before starting so UI builds
/// and instrumentation changes are reflected on-device. If no local binary is
/// provided, fall back to starting the existing remote daemon.
///
/// * `local_path` – local binary to push (empty string to skip push)
/// * `remote_path` – absolute path on device (e.g. `/data/local/tmp/realtime_profile`)
/// * `grpc_port` – port the daemon will listen on
pub async fn deploy_and_start(
    serial: &str,
    local_path: &str,
    remote_path: &str,
    grpc_port: u16,
    low_overhead: bool,
) -> Result<()> {
    if !local_path.is_empty() {
        log::info!("[{serial}] 推送最新 daemon binary 後啟動");
        kill_daemon(serial).await;
        push_binaries(serial, local_path, remote_path).await?;
        start_daemon(serial, remote_path, grpc_port, low_overhead).await?;

        if !is_running(serial).await {
            anyhow::bail!("部署後 daemon 仍無法啟動");
        }
        log::info!("[{serial}] daemon 已使用最新 binary 啟動");
        return Ok(());
    }

    log::info!("[{serial}] 未提供 local binary，嘗試啟動既有 remote daemon");
    start_daemon(serial, remote_path, grpc_port, low_overhead).await?;

    if !is_running(serial).await {
        anyhow::bail!("Daemon 啟動失敗且未提供 local binary 路徑");
    }
    log::info!("[{serial}] 既有 remote daemon 啟動成功");
    Ok(())
}

/// Check if the daemon is already running on the device.
pub async fn is_running(serial: &str) -> bool {
    match commands::shell(serial, "pidof realtime_profile").await {
        Ok(output) => !output.trim().is_empty(),
        Err(_) => false,
    }
}

/// Return the daemon PID if `realtime_profile` is running.
pub async fn get_daemon_pid(serial: &str) -> Option<i32> {
    let pid_str = commands::shell(serial, "pidof realtime_profile")
        .await
        .ok()?;
    pid_str
        .trim()
        .split_whitespace()
        .next()
        .and_then(|s| s.parse::<i32>().ok())
}

/// Return daemon uid (effective uid field in `/proc/<pid>/status`), if available.
pub async fn get_daemon_uid(serial: &str) -> Option<u32> {
    let pid = get_daemon_pid(serial).await?;
    let status = commands::shell(serial, &format!("cat /proc/{pid}/status"))
        .await
        .ok()?;
    for line in status.lines() {
        if line.starts_with("Uid:") {
            // Format: Uid:\tReal\tEffective\tSavedSet\tFilesystem
            let mut parts = line.split_whitespace();
            let _label = parts.next();
            if let Some(effective_uid) = parts.next() {
                return effective_uid.parse::<u32>().ok();
            }
        }
    }
    None
}

/// Kill the daemon, using `su -c pkill` as fallback if regular pkill fails.
async fn kill_daemon(serial: &str) {
    let _ = commands::shell(serial, "pkill -f realtime_profile").await;
    if is_running(serial).await {
        log::info!("[{serial}] pkill 未能殺掉 daemon，嘗試 su -c pkill");
        let _ = commands::shell(serial, "su -c \"pkill -f realtime_profile\"").await;
    }
    // Poll until process is dead (up to 2s)
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    loop {
        sleep(Duration::from_millis(100)).await;
        if !is_running(serial).await {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            log::warn!("kill_daemon wait timed out after 2s");
            break;
        }
    }
}

/// Ensure daemon is running as root. If a non-root daemon exists, restart it.
pub async fn ensure_running_rooted(
    serial: &str,
    local_path: &str,
    remote_path: &str,
    grpc_port: u16,
    low_overhead: bool,
) -> Result<()> {
    if !is_running(serial).await {
        log::info!("[{serial}] daemon 未運行，啟動中...");
        return deploy_and_start(serial, local_path, remote_path, grpc_port, low_overhead).await;
    }

    match get_daemon_uid(serial).await {
        Some(0) => match daemon_low_overhead_matches(serial, low_overhead).await {
            Some(true) => {
                log::info!("[{serial}] daemon 已為 root，沿用現有進程");
                Ok(())
            }
            Some(false) => {
                log::info!("[{serial}] daemon low-overhead 設定已變更，重啟套用...");
                kill_daemon(serial).await;
                deploy_and_start(serial, local_path, remote_path, grpc_port, low_overhead).await
            }
            None => {
                log::warn!("[{serial}] 無法判斷 daemon low-overhead 設定，重啟套用...");
                kill_daemon(serial).await;
                deploy_and_start(serial, local_path, remote_path, grpc_port, low_overhead).await
            }
        },
        Some(uid) => {
            log::warn!("[{serial}] daemon uid={uid} (非 root)，重啟為 root...");
            kill_daemon(serial).await;
            deploy_and_start(serial, local_path, remote_path, grpc_port, low_overhead).await
        }
        None => {
            log::warn!("[{serial}] 無法判斷 daemon uid，保守重啟為 root...");
            kill_daemon(serial).await;
            deploy_and_start(serial, local_path, remote_path, grpc_port, low_overhead).await
        }
    }
}

async fn daemon_low_overhead_matches(serial: &str, expected_low_overhead: bool) -> Option<bool> {
    let pid_str = commands::shell(serial, "pidof realtime_profile")
        .await
        .ok()?;
    let pid = pid_str.trim().split_whitespace().next()?;
    if pid.is_empty() {
        return None;
    }

    let cmdline = commands::shell(serial, &format!("cat /proc/{pid}/cmdline | tr '\\0' ' '"))
        .await
        .ok()?;
    let parts: Vec<&str> = cmdline.split_whitespace().collect();
    let has_low_overhead = parts.iter().any(|part| *part == "--low-overhead");
    let has_sync_only = parts.iter().any(|part| *part == "--sync-only");
    let has_legacy_mode = parts.iter().any(|part| *part == "--legacy-mode");

    Some(if expected_low_overhead {
        has_low_overhead && has_sync_only && !has_legacy_mode
    } else {
        has_legacy_mode || !has_low_overhead
    })
}

/// Query the gRPC port that the running daemon is actually listening on.
///
/// Reads `/proc/<pid>/cmdline` and looks for the `--grpc-port` argument.
/// Returns `None` if the daemon is not running or the port cannot be determined.
pub async fn get_grpc_port(serial: &str) -> Option<u16> {
    let pid_str = commands::shell(serial, "pidof realtime_profile")
        .await
        .ok()?;
    let pid = pid_str.trim().split_whitespace().next()?;
    if pid.is_empty() {
        return None;
    }
    let cmdline = commands::shell(serial, &format!("cat /proc/{pid}/cmdline | tr '\\0' ' '"))
        .await
        .ok()?;
    let parts: Vec<&str> = cmdline.split_whitespace().collect();
    for (i, part) in parts.iter().enumerate() {
        if *part == "--grpc-port" {
            return parts.get(i + 1).and_then(|s| s.parse().ok());
        }
    }
    None
}
