use std::ffi::c_char;
use std::os::raw::{c_double, c_int, c_uint};
use std::sync::OnceLock;

use libloading::{Library, Symbol};

use super::constants::TaskHandle;
use super::error::{DaqmxError, DaqmxResult};

static DAQMX_LIB: OnceLock<Result<DaqmxLib, String>> = OnceLock::new();

/// Callback type for DAQmxRegisterEveryNSamplesEvent
pub type EveryNSamplesCallback = unsafe extern "C" fn(
    task_handle: TaskHandle,
    event_type: c_int,
    n_samples: c_uint,
    callback_data: *mut std::ffi::c_void,
) -> c_int;

pub struct DaqmxLib {
    _lib: Library,
    // Task lifecycle
    pub create_task: unsafe extern "C" fn(*const c_char, *mut TaskHandle) -> c_int,
    pub start_task: unsafe extern "C" fn(TaskHandle) -> c_int,
    pub stop_task: unsafe extern "C" fn(TaskHandle) -> c_int,
    pub clear_task: unsafe extern "C" fn(TaskHandle) -> c_int,
    #[allow(dead_code)]
    pub is_task_done: unsafe extern "C" fn(TaskHandle, *mut c_uint) -> c_int,
    // Channels
    pub create_ai_voltage_chan: unsafe extern "C" fn(
        TaskHandle,
        *const c_char,
        *const c_char,
        c_int,
        c_double,
        c_double,
        c_int,
        *const c_char,
    ) -> c_int,
    // Timing
    pub cfg_samp_clk_timing:
        unsafe extern "C" fn(TaskHandle, *const c_char, c_double, c_int, c_int, u64) -> c_int,
    // Read
    pub read_analog_f64: unsafe extern "C" fn(
        TaskHandle,
        c_int,
        c_double,
        c_int,
        *mut c_double,
        c_uint,
        *mut c_int,
        *mut c_uint,
    ) -> c_int,
    // Register callback
    pub register_every_n_samples_event: unsafe extern "C" fn(
        TaskHandle,
        c_int,
        c_uint,
        c_uint,
        EveryNSamplesCallback,
        *mut std::ffi::c_void,
    ) -> c_int,
    // System queries
    pub get_sys_dev_names: unsafe extern "C" fn(*mut c_char, c_uint) -> c_int,
    pub get_dev_ai_physical_chans:
        unsafe extern "C" fn(*const c_char, *mut c_char, c_uint) -> c_int,
    pub get_dev_product_type: unsafe extern "C" fn(*const c_char, *mut c_char, c_uint) -> c_int,
    pub get_dev_serial_num: unsafe extern "C" fn(*const c_char, *mut c_uint) -> c_int,
    // Scale
    pub create_lin_scale:
        unsafe extern "C" fn(*const c_char, c_double, c_double, c_int, *const c_char) -> c_int,
    // Buffer
    pub cfg_input_buffer: unsafe extern "C" fn(TaskHandle, c_uint) -> c_int,
    // Trigger
    pub cfg_dig_edge_start_trig: unsafe extern "C" fn(TaskHandle, *const c_char, c_int) -> c_int,
    // Logging
    pub configure_logging:
        unsafe extern "C" fn(TaskHandle, *const c_char, c_int, *const c_char, c_int) -> c_int,
    // Device control
    pub self_cal: unsafe extern "C" fn(*const c_char) -> c_int,
    pub reset_device: unsafe extern "C" fn(*const c_char) -> c_int,
    // Error
    pub get_extended_error_info: unsafe extern "C" fn(*mut c_char, c_uint) -> c_int,
}

// Safety: DaqmxLib stores function pointers from a loaded DLL. The NI-DAQmx
// DLL functions are thread-safe per NI documentation.
unsafe impl Send for DaqmxLib {}
unsafe impl Sync for DaqmxLib {}

impl DaqmxLib {
    /// Get or load the singleton DaqmxLib instance.
    /// The library is loaded once and never unloaded, because NI-DAQmx
    /// does not support being unloaded and reloaded within the same process.
    pub fn get_or_load() -> DaqmxResult<&'static Self> {
        let result = DAQMX_LIB.get_or_init(|| Self::load_inner().map_err(|e| e.to_string()));
        match result {
            Ok(lib) => Ok(lib),
            Err(e) => Err(DaqmxError::LibraryNotFound(e.clone())),
        }
    }

    fn load_inner() -> DaqmxResult<Self> {
        let lib = unsafe { Library::new("nicaiu.dll") }.map_err(|e| {
            DaqmxError::LibraryNotFound(format!(
                "Cannot load NI-DAQmx driver (nicaiu.dll). Install NI-DAQmx from ni.com: {}",
                e
            ))
        })?;

        unsafe {
            macro_rules! load_fn {
                ($lib:expr, $name:expr, $ty:ty) => {{
                    let sym: Symbol<$ty> = $lib.get($name).map_err(|e| {
                        DaqmxError::LibraryNotFound(format!(
                            "Symbol {} not found: {}",
                            String::from_utf8_lossy($name),
                            e
                        ))
                    })?;
                    *sym.into_raw()
                }};
            }

            type FnCreateTask = unsafe extern "C" fn(*const c_char, *mut TaskHandle) -> c_int;
            type FnTaskOnly = unsafe extern "C" fn(TaskHandle) -> c_int;
            type FnIsTaskDone = unsafe extern "C" fn(TaskHandle, *mut c_uint) -> c_int;
            type FnCreateAIVoltageChan = unsafe extern "C" fn(
                TaskHandle,
                *const c_char,
                *const c_char,
                c_int,
                c_double,
                c_double,
                c_int,
                *const c_char,
            ) -> c_int;
            type FnCfgSampClkTiming = unsafe extern "C" fn(
                TaskHandle,
                *const c_char,
                c_double,
                c_int,
                c_int,
                u64,
            ) -> c_int;
            type FnReadAnalogF64 = unsafe extern "C" fn(
                TaskHandle,
                c_int,
                c_double,
                c_int,
                *mut c_double,
                c_uint,
                *mut c_int,
                *mut c_uint,
            ) -> c_int;
            type FnRegisterEveryN = unsafe extern "C" fn(
                TaskHandle,
                c_int,
                c_uint,
                c_uint,
                EveryNSamplesCallback,
                *mut std::ffi::c_void,
            ) -> c_int;
            type FnGetSysDevNames = unsafe extern "C" fn(*mut c_char, c_uint) -> c_int;
            type FnGetDevStringProp =
                unsafe extern "C" fn(*const c_char, *mut c_char, c_uint) -> c_int;
            type FnGetDevSerialNum = unsafe extern "C" fn(*const c_char, *mut c_uint) -> c_int;
            type FnCreateLinScale = unsafe extern "C" fn(
                *const c_char,
                c_double,
                c_double,
                c_int,
                *const c_char,
            ) -> c_int;
            type FnCfgInputBuffer = unsafe extern "C" fn(TaskHandle, c_uint) -> c_int;
            type FnCfgDigEdgeStartTrig =
                unsafe extern "C" fn(TaskHandle, *const c_char, c_int) -> c_int;
            type FnConfigureLogging = unsafe extern "C" fn(
                TaskHandle,
                *const c_char,
                c_int,
                *const c_char,
                c_int,
            ) -> c_int;
            type FnDeviceString = unsafe extern "C" fn(*const c_char) -> c_int;
            type FnGetExtendedErrorInfo = unsafe extern "C" fn(*mut c_char, c_uint) -> c_int;

            let daqmx = DaqmxLib {
                create_task: load_fn!(lib, b"DAQmxCreateTask\0", FnCreateTask),
                start_task: load_fn!(lib, b"DAQmxStartTask\0", FnTaskOnly),
                stop_task: load_fn!(lib, b"DAQmxStopTask\0", FnTaskOnly),
                clear_task: load_fn!(lib, b"DAQmxClearTask\0", FnTaskOnly),
                is_task_done: load_fn!(lib, b"DAQmxIsTaskDone\0", FnIsTaskDone),
                create_ai_voltage_chan: load_fn!(
                    lib,
                    b"DAQmxCreateAIVoltageChan\0",
                    FnCreateAIVoltageChan
                ),
                cfg_samp_clk_timing: load_fn!(lib, b"DAQmxCfgSampClkTiming\0", FnCfgSampClkTiming),
                read_analog_f64: load_fn!(lib, b"DAQmxReadAnalogF64\0", FnReadAnalogF64),
                register_every_n_samples_event: load_fn!(
                    lib,
                    b"DAQmxRegisterEveryNSamplesEvent\0",
                    FnRegisterEveryN
                ),
                get_sys_dev_names: load_fn!(lib, b"DAQmxGetSysDevNames\0", FnGetSysDevNames),
                get_dev_ai_physical_chans: load_fn!(
                    lib,
                    b"DAQmxGetDevAIPhysicalChans\0",
                    FnGetDevStringProp
                ),
                get_dev_product_type: load_fn!(
                    lib,
                    b"DAQmxGetDevProductType\0",
                    FnGetDevStringProp
                ),
                get_dev_serial_num: load_fn!(lib, b"DAQmxGetDevSerialNum\0", FnGetDevSerialNum),
                create_lin_scale: load_fn!(lib, b"DAQmxCreateLinScale\0", FnCreateLinScale),
                cfg_input_buffer: load_fn!(lib, b"DAQmxCfgInputBuffer\0", FnCfgInputBuffer),
                cfg_dig_edge_start_trig: load_fn!(
                    lib,
                    b"DAQmxCfgDigEdgeStartTrig\0",
                    FnCfgDigEdgeStartTrig
                ),
                configure_logging: load_fn!(lib, b"DAQmxConfigureLogging\0", FnConfigureLogging),
                self_cal: load_fn!(lib, b"DAQmxSelfCal\0", FnDeviceString),
                reset_device: load_fn!(lib, b"DAQmxResetDevice\0", FnDeviceString),
                get_extended_error_info: load_fn!(
                    lib,
                    b"DAQmxGetExtendedErrorInfo\0",
                    FnGetExtendedErrorInfo
                ),
                _lib: lib,
            };
            Ok(daqmx)
        }
    }

    pub fn get_error_string(&self) -> String {
        let mut buf = vec![0u8; 2048];
        unsafe {
            (self.get_extended_error_info)(buf.as_mut_ptr() as *mut c_char, buf.len() as c_uint);
        }
        let nul_pos = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
        String::from_utf8_lossy(&buf[..nul_pos]).to_string()
    }

    pub fn check(&self, code: i32) -> DaqmxResult<()> {
        super::error::check_error(code, &|| self.get_error_string())
    }
}
