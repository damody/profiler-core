use std::ffi::CString;

use super::constants::*;
use super::error::DaqmxResult;
use super::ffi_daqmx::DaqmxLib;

pub struct DaqTask<'a> {
    pub lib: &'a DaqmxLib,
    pub handle: TaskHandle,
}

impl<'a> DaqTask<'a> {
    pub fn new(lib: &'a DaqmxLib, name: &str) -> DaqmxResult<Self> {
        let name_c = CString::new(name).unwrap();
        let mut handle: TaskHandle = 0;
        let code = unsafe { (lib.create_task)(name_c.as_ptr(), &mut handle) };
        lib.check(code)?;
        Ok(Self { lib, handle })
    }

    pub fn add_ai_voltage_chan(
        &self,
        physical_channel: &str,
        name_to_assign: &str,
        terminal_config: i32,
        min_val: f64,
        max_val: f64,
        use_custom_scale: bool,
        custom_scale_name: &str,
    ) -> DaqmxResult<()> {
        let phys_c = CString::new(physical_channel).unwrap();
        let name_c = CString::new(name_to_assign).unwrap();
        let (units, scale_c) = if use_custom_scale {
            (
                DAQmx_Val_FromCustomScale,
                CString::new(custom_scale_name).unwrap(),
            )
        } else {
            (DAQmx_Val_Volts, CString::new("").unwrap())
        };
        let code = unsafe {
            (self.lib.create_ai_voltage_chan)(
                self.handle,
                phys_c.as_ptr(),
                name_c.as_ptr(),
                terminal_config,
                min_val,
                max_val,
                units,
                scale_c.as_ptr(),
            )
        };
        self.lib.check(code)
    }

    pub fn cfg_timing(&self, rate: f64, sample_mode: i32, samps_per_chan: u64) -> DaqmxResult<()> {
        let source = CString::new("").unwrap();
        let code = unsafe {
            (self.lib.cfg_samp_clk_timing)(
                self.handle,
                source.as_ptr(),
                rate,
                DAQmx_Val_Rising,
                sample_mode,
                samps_per_chan,
            )
        };
        self.lib.check(code)
    }

    pub fn start(&self) -> DaqmxResult<()> {
        let code = unsafe { (self.lib.start_task)(self.handle) };
        self.lib.check(code)
    }

    #[allow(dead_code)]
    pub fn stop(&self) -> DaqmxResult<()> {
        let code = unsafe { (self.lib.stop_task)(self.handle) };
        self.lib.check(code)
    }

    pub fn cfg_input_buffer(&self, num_samps_per_chan: u32) -> DaqmxResult<()> {
        let code = unsafe { (self.lib.cfg_input_buffer)(self.handle, num_samps_per_chan) };
        self.lib.check(code)
    }

    #[allow(dead_code)]
    pub fn cfg_pfi12_trigger(&self) -> DaqmxResult<()> {
        let source = CString::new("/Dev1/PFI12").unwrap();
        let code = unsafe {
            (self.lib.cfg_dig_edge_start_trig)(self.handle, source.as_ptr(), DAQmx_Val_Rising)
        };
        self.lib.check(code)
    }

    #[allow(dead_code)]
    pub fn configure_logging(&self, file_path: &str, logging_mode: i32) -> DaqmxResult<()> {
        let path_c = CString::new(file_path).unwrap();
        let group = CString::new("").unwrap();
        let code = unsafe {
            (self.lib.configure_logging)(
                self.handle,
                path_c.as_ptr(),
                logging_mode,
                group.as_ptr(),
                DAQmx_Val_OpenOrCreate,
            )
        };
        self.lib.check(code)
    }
}

impl Drop for DaqTask<'_> {
    fn drop(&mut self) {
        unsafe {
            (self.lib.clear_task)(self.handle);
        }
    }
}
