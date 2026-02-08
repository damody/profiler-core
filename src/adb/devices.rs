use anyhow::{Context, Result};
use regex::Regex;

use super::DeviceInfo;

/// Run `adb devices -l` and parse the output into a list of `DeviceInfo`.
pub async fn get_devices() -> Result<Vec<DeviceInfo>> {
    let output = super::adb_command()
        .args(["devices", "-l"])
        .output()
        .await
        .context("Failed to execute adb devices -l")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("adb devices failed: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_devices_output(&stdout)
}

fn parse_devices_output(output: &str) -> Result<Vec<DeviceInfo>> {
    // Each device line looks like:
    //   SERIAL           STATE   usb:... product:... model:MODEL device:...
    // or simply:
    //   SERIAL           STATE
    let model_re = Regex::new(r"model:(\S+)").unwrap();
    let mut devices = Vec::new();

    for line in output.lines() {
        let line = line.trim();
        // Skip the header and blank lines
        if line.is_empty() || line.starts_with("List of") || line.starts_with("*") {
            continue;
        }

        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 2 {
            continue;
        }

        let serial = parts[0].to_string();
        let state = parts[1].to_string();

        let model = model_re
            .captures(line)
            .and_then(|cap| cap.get(1))
            .map(|m| m.as_str().replace('_', " "))
            .unwrap_or_default();

        devices.push(DeviceInfo {
            serial,
            model,
            state,
        });
    }

    Ok(devices)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_devices_output() {
        let output = "\
List of devices attached
R5CR1234567         device usb:1-1 product:gts9uwifi model:SM_X710 device:gts9uwifi transport_id:1
192.168.1.100:5555  device product:phone model:Pixel_7 device:panther transport_id:2

";
        let devices = parse_devices_output(output).unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].serial, "R5CR1234567");
        assert_eq!(devices[0].state, "device");
        assert_eq!(devices[0].model, "SM X710");
        assert_eq!(devices[1].serial, "192.168.1.100:5555");
        assert_eq!(devices[1].model, "Pixel 7");
    }
}
