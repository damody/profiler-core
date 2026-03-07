use anyhow::{Context, Result};

/// Run `adb -s <serial> shell <command>` and return stdout.
pub async fn shell(serial: &str, command: &str) -> Result<String> {
    let output = super::adb_command()
        .args(["-s", serial, "shell", command])
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
        .args(["-s", serial, "push", "-z", "zstd", local, remote])
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
        .args(["-s", serial, "pull", "-z", "zstd", remote, local])
        .output()
        .await
        .context("Failed to execute adb pull")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("adb pull failed: {stderr}");
    }

    Ok(())
}

/// Run `adb -s <serial> root` to restart adbd as root.
pub async fn root(serial: &str) -> Result<String> {
    let output = super::adb_command()
        .args(["-s", serial, "root"])
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
        .args(["-s", serial, "tcpip", &port.to_string()])
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
    let output = super::adb_command()
        .args([
            "-s",
            serial,
            "forward",
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

    Ok(())
}
