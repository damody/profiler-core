use std::ffi::CString;

use super::constants::DAQmx_Val_Volts;
use super::error::DaqmxResult;
use super::ffi_daqmx::DaqmxLib;

/// Create a linear scale: scaled = raw * slope + y_intercept
pub fn create_linear_scale(
    lib: &DaqmxLib,
    name: &str,
    slope: f64,
    y_intercept: f64,
) -> DaqmxResult<()> {
    let name_c = CString::new(name).unwrap();
    let units_c = CString::new("").unwrap();
    let code = unsafe {
        (lib.create_lin_scale)(
            name_c.as_ptr(),
            slope,
            y_intercept,
            DAQmx_Val_Volts,
            units_c.as_ptr(),
        )
    };
    lib.check(code)
}
