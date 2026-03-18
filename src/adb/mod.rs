pub mod devices;
pub mod commands;
pub mod daemon;

use tokio::process::Command;

/// 建立已設定平台旗標的 adb Command。
/// Windows 上設定 CREATE_NO_WINDOW 避免 CMD 視窗閃爍。
pub(crate) fn adb_command() -> Command {
    let mut cmd = Command::new("adb");
    #[cfg(windows)]
    {
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
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
