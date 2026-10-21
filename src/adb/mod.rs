pub mod commands;
pub mod daemon;
pub mod devices;

use tokio::process::Command;

/// Select an unambiguous ADB transport when a device key came from a duplicate
/// serial listing. A stale transport fails instead of targeting another phone.
pub fn device_selector(serial: &str) -> [&str; 2] {
    match serial.strip_prefix("transport:") {
        Some(id) if !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()) => ["-t", id],
        _ => ["-s", serial],
    }
}

#[cfg(test)]
mod selector_tests {
    use super::device_selector;

    #[test]
    fn duplicate_serial_keys_select_a_transport() {
        assert_eq!(device_selector("transport:19"), ["-t", "19"]);
        assert_eq!(device_selector("transport:20"), ["-t", "20"]);
        assert_eq!(device_selector("phone"), ["-s", "phone"]);
        assert_eq!(device_selector("transport:"), ["-s", "transport:"]);
        assert_eq!(device_selector("transport:abc"), ["-s", "transport:abc"]);
    }
}

/// 建立已設定平台旗標的 adb Command。
/// Windows 上設定 CREATE_NO_WINDOW 避免 CMD 視窗閃爍。
pub(crate) fn adb_command() -> Command {
    #[cfg(windows)]
    {
        let mut cmd = Command::new("adb");
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
        cmd
    }

    #[cfg(not(windows))]
    {
        Command::new("adb")
    }
}

/// Information about a single ADB device.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub serial: String,
    pub model: String,
    pub state: String,
    pub product: String,
    pub device_name: String,
}
