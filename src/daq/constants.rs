// DAQmx constants matching the NI-DAQmx C API values.
// Reference: https://www.ni.com/docs/en-US/bundle/ni-daqmx-c-api-ref/
#![allow(non_upper_case_globals, dead_code)]

// Terminal configurations
pub const DAQmx_Val_Cfg_Default: i32 = -1;
pub const DAQmx_Val_RSE: i32 = 10083;
pub const DAQmx_Val_NRSE: i32 = 10078;
pub const DAQmx_Val_Diff: i32 = 10106;
pub const DAQmx_Val_PseudoDiff: i32 = 12529;

// Acquisition types
pub const DAQmx_Val_FiniteSamps: i32 = 10178;
pub const DAQmx_Val_ContSamps: i32 = 10123;
pub const DAQmx_Val_HWTimedSinglePoint: i32 = 12522;

// Voltage units
pub const DAQmx_Val_Volts: i32 = 10348;
pub const DAQmx_Val_FromCustomScale: i32 = 10065;

// Pre-scaled units
pub const DAQmx_Val_VoltsPerVolt: i32 = 15896;

// Logging mode
pub const DAQmx_Val_Log: i32 = 15844;
pub const DAQmx_Val_LogAndRead: i32 = 15842;
pub const DAQmx_Val_Off: i32 = 10231;

// Logging operation
pub const DAQmx_Val_Open: i32 = 10437;
pub const DAQmx_Val_OpenOrCreate: i32 = 15846;
pub const DAQmx_Val_CreateOrReplace: i32 = 15847;
pub const DAQmx_Val_Create: i32 = 15848;

// Edge
pub const DAQmx_Val_Rising: i32 = 10280;
pub const DAQmx_Val_Falling: i32 = 10171;

// Read fill mode
pub const DAQmx_Val_GroupByChannel: i32 = 0;
pub const DAQmx_Val_GroupByScanNumber: i32 = 1;

// Timeout
pub const DAQmx_Val_WaitInfinitely: f64 = -1.0;

// EveryNSamples event type
pub const DAQmx_Val_Acquired_Into_Buffer: i32 = 1;

// Task handle type
pub type TaskHandle = usize;
