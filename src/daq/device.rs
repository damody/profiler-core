use std::ffi::CString;

use log::{debug, info};

use super::error::DaqmxResult;
use super::ffi_daqmx::DaqmxLib;

#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub product_type: String,
    pub serial_number: u32,
    pub ai_channels: Vec<String>,
}

fn parse_daqmx_list(s: &str) -> Vec<String> {
    s.split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

fn query_string(
    func: unsafe extern "C" fn(
        *const std::os::raw::c_char,
        *mut std::os::raw::c_char,
        std::os::raw::c_uint,
    ) -> std::os::raw::c_int,
    lib: &DaqmxLib,
    device_name: &str,
) -> DaqmxResult<String> {
    let dev_cstr = CString::new(device_name).unwrap();
    let mut buf = vec![0u8; 4096];
    let code = unsafe {
        func(
            dev_cstr.as_ptr(),
            buf.as_mut_ptr() as *mut _,
            buf.len() as u32,
        )
    };
    lib.check(code)?;
    let nul_pos = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    Ok(String::from_utf8_lossy(&buf[..nul_pos]).to_string())
}

pub fn enumerate_devices(lib: &DaqmxLib) -> DaqmxResult<Vec<DeviceInfo>> {
    let mut buf = vec![0u8; 4096];
    let code = unsafe { (lib.get_sys_dev_names)(buf.as_mut_ptr() as *mut _, buf.len() as u32) };
    lib.check(code)?;
    let nul_pos = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    let names_str = String::from_utf8_lossy(&buf[..nul_pos]).to_string();
    let dev_names = parse_daqmx_list(&names_str);

    if dev_names.is_empty() {
        info!("DAQ: No NI-DAQmx devices detected");
        return Err(super::error::DaqmxError::NoDevices);
    }

    debug!("DAQ: Found device names: {:?}", dev_names);

    let mut devices = Vec::new();
    for name in &dev_names {
        let product_type = query_string(lib.get_dev_product_type, lib, name)?;

        let dev_cstr = CString::new(name.as_str()).unwrap();
        let mut serial: u32 = 0;
        let code = unsafe { (lib.get_dev_serial_num)(dev_cstr.as_ptr(), &mut serial) };
        lib.check(code)?;

        let channels_str = query_string(lib.get_dev_ai_physical_chans, lib, name)?;
        let ai_channels = parse_daqmx_list(&channels_str);

        info!(
            "DAQ: Device '{}': {} (S/N: 0x{:08X}, {} AI channels)",
            name,
            product_type,
            serial,
            ai_channels.len()
        );

        devices.push(DeviceInfo {
            name: name.clone(),
            product_type,
            serial_number: serial,
            ai_channels,
        });
    }
    Ok(devices)
}
