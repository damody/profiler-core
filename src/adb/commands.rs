use anyhow::{Context, Result};

/// Run `adb -s <serial> shell <command>` and return stdout.
pub async fn shell(serial: &str, command: &str) -> Result<String> {
    let output = super::adb_command()
        .args(super::device_selector(serial))
        .args(["shell", command])
        .output()
        .await
        .context("Failed to execute adb shell")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("adb shell failed: {stderr}");
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Push a local file to the device.
pub async fn push(serial: &str, local: &str, remote: &str) -> Result<()> {
    let output = super::adb_command()
        .args(super::device_selector(serial))
        .args(["push", "-z", "zstd", local, remote])
        .output()
        .await
        .context("Failed to execute adb push")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("adb push failed: {stderr}");
    }

    Ok(())
}

/// Pull a remote file from the device to local.
pub async fn pull(serial: &str, remote: &str, local: &str) -> Result<()> {
    let output = super::adb_command()
        .args(super::device_selector(serial))
        .args(["pull", "-z", "zstd", remote, local])
        .output()
        .await
        .context("Failed to execute adb pull")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("adb pull failed: {stderr}");
    }

    Ok(())
}

/// Run `adb -s <serial> remount` to remount partitions as read-write.
pub async fn remount(serial: &str) -> Result<String> {
    let output = super::adb_command()
        .args(super::device_selector(serial))
        .arg("remount")
        .output()
        .await
        .context("Failed to execute adb remount")?;
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    // remount 的有用訊息可能在 stdout 或 stderr
    if !stderr.is_empty() && stdout.is_empty() {
        Ok(stderr)
    } else {
        Ok(stdout)
    }
}

/// Run `adb -s <serial> root` to restart adbd as root.
pub async fn root(serial: &str) -> Result<String> {
    let output = super::adb_command()
        .args(super::device_selector(serial))
        .arg("root")
        .output()
        .await
        .context("Failed to execute adb root")?;

    // adb root may return non-zero on some devices; treat output as best-effort
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Check whether the current adb shell runs as root (uid=0).
pub async fn is_root_shell(serial: &str) -> Result<bool> {
    let id_output = shell(serial, "id").await?;
    Ok(id_output.contains("uid=0"))
}

/// Run `adb disconnect <serial>` to disconnect a specific WiFi-connected device.
pub async fn disconnect(serial: &str) -> Result<String> {
    let output = super::adb_command()
        .args(["disconnect", serial])
        .output()
        .await
        .context("Failed to execute adb disconnect")?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run `adb disconnect` to disconnect all WiFi-connected devices.
pub async fn disconnect_all() -> Result<String> {
    let output = super::adb_command()
        .arg("disconnect")
        .output()
        .await
        .context("Failed to execute adb disconnect")?;

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run `adb -s <serial> tcpip <port>` to restart adbd in TCP/IP mode.
pub async fn tcpip(serial: &str, port: u16) -> Result<String> {
    let output = super::adb_command()
        .args(super::device_selector(serial))
        .args(["tcpip", &port.to_string()])
        .output()
        .await
        .context("Failed to execute adb tcpip")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("adb tcpip failed: {stderr}");
    }

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Run `adb connect <ip>:<port>` and verify the connection succeeded.
pub async fn connect_device(ip: &str, port: u16) -> Result<String> {
    let target = format!("{ip}:{port}");
    let output = super::adb_command()
        .args(["connect", &target])
        .output()
        .await
        .context("Failed to execute adb connect")?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stdout.contains("cannot connect") || stdout.contains("failed to connect") {
        anyhow::bail!("adb connect failed: {stdout}");
    }

    Ok(stdout)
}

/// Set up ADB port forwarding: local TCP port -> device TCP port.
pub async fn forward(serial: &str, local_port: u16, remote_port: u16) -> Result<()> {
    static OWNED: std::sync::OnceLock<std::sync::Mutex<std::collections::HashSet<(String, u16, u16)>>> = std::sync::OnceLock::new();
    let owners = OWNED.get_or_init(Default::default);
    let key = (serial.to_owned(), local_port, remote_port);
    let listing = super::adb_command().args(super::device_selector(serial)).args(["forward", "--list"]).output().await?;
    let local = format!("tcp:{local_port}");
    let remote = format!("tcp:{remote_port}");
    if listing.status.success() {
        for line in String::from_utf8_lossy(&listing.stdout).lines() {
            let fields: Vec<_> = line.split_whitespace().collect();
            if fields.len() == 3 && fields[1] == local {
                // A transport-key's raw serial may be duplicated; don't assume ownership.
                // --no-rebind below fails closed in that case. Existing connections are reused by the caller.
                if fields[2] == remote && ((!serial.starts_with("transport:") && fields[0] == serial) || owners.lock().unwrap().contains(&key)) { return Ok(()); }
            }
        }
    }
    let output = super::adb_command()
        .args(super::device_selector(serial))
        .args([
            "forward",
            "--no-rebind",
            &format!("tcp:{local_port}"),
            &format!("tcp:{remote_port}"),
        ])
        .output()
        .await
        .context("Failed to execute adb forward")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("adb forward failed: {stderr}");
    }

    owners.lock().unwrap().insert(key);
    Ok(())
}

/// Set up ADB reverse forwarding: device TCP port -> host TCP port.
pub async fn reverse(serial: &str, device_port: u16, host_port: u16) -> Result<()> {
    let output = super::adb_command()
        .args(super::device_selector(serial))
        .args([
            "reverse",
            &format!("tcp:{device_port}"),
            &format!("tcp:{host_port}"),
        ])
        .output()
        .await
        .context("Failed to execute adb reverse")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("adb reverse failed: {stderr}");
    }
    Ok(())
}

/// Remove one ADB reverse forwarding entry.
pub async fn reverse_remove(serial: &str, device_port: u16) -> Result<()> {
    let output = super::adb_command()
        .args(super::device_selector(serial))
        .args(["reverse", "--remove", &format!("tcp:{device_port}")])
        .output()
        .await
        .context("Failed to remove adb reverse")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("adb reverse --remove failed: {stderr}");
    }
    Ok(())
}
