use std::cell::UnsafeCell;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::constants::*;
use super::error::DaqmxResult;
use super::ffi_daqmx::DaqmxLib;
use super::task::DaqTask;

/// Buffer that accumulates samples from EveryNSamples callback
pub struct AccumulationBuffer {
    pub data: Vec<Vec<f64>>,
    pub total_samples: usize,
}

impl AccumulationBuffer {
    pub fn new(num_channels: usize) -> Self {
        Self {
            data: (0..num_channels).map(|_| Vec::new()).collect(),
            total_samples: 0,
        }
    }

    /// Take all accumulated samples, leaving empty vecs behind
    pub fn take(&mut self) -> Vec<Vec<f64>> {
        self.data
            .iter_mut()
            .map(|ch| std::mem::take(ch))
            .collect()
    }
}

pub struct CallbackContext {
    lib: *const DaqmxLib,
    handle: TaskHandle,
    pub buffer: Arc<Mutex<AccumulationBuffer>>,
    num_channels: usize,
    /// Pre-allocated read buffer reused across callbacks to avoid per-call allocation.
    /// Safety: only accessed from the single DAQmx callback thread.
    read_buf: UnsafeCell<Vec<f64>>,
    pub error_count: AtomicU64,
    pub lock_fail_count: AtomicU64,
}

// Safety: DaqmxLib function pointers are Send-safe as they point to
// statically loaded DLL functions that are thread-safe per NI-DAQmx spec.
unsafe impl Send for CallbackContext {}
unsafe impl Sync for CallbackContext {}

/// The C callback function for EveryNSamples
unsafe extern "C" fn every_n_callback(
    _task_handle: TaskHandle,
    _event_type: std::os::raw::c_int,
    n_samples: std::os::raw::c_uint,
    callback_data: *mut std::ffi::c_void,
) -> std::os::raw::c_int {
    let ctx = &*(callback_data as *const CallbackContext);
    let total_read = (ctx.num_channels as u32) * n_samples;

    // Reuse pre-allocated buffer; resize only if needed (should not happen in steady state)
    let read_buf = &mut *ctx.read_buf.get();
    let needed = total_read as usize;
    if read_buf.len() < needed {
        read_buf.resize(needed, 0.0);
    }
    // Zero the portion we'll use
    read_buf[..needed].fill(0.0);

    let mut samples_read: i32 = 0;

    let code = ((*ctx.lib).read_analog_f64)(
        ctx.handle,
        n_samples as i32,
        DAQmx_Val_WaitInfinitely,
        DAQmx_Val_GroupByChannel,
        read_buf.as_mut_ptr(),
        total_read,
        &mut samples_read,
        std::ptr::null_mut(),
    );

    if code >= 0 {
        match ctx.buffer.lock() {
            Ok(mut buf) => {
                for ch in 0..ctx.num_channels {
                    let start = ch * n_samples as usize;
                    let end = start + samples_read as usize;
                    buf.data[ch].extend_from_slice(&read_buf[start..end]);
                }
                buf.total_samples += samples_read as usize;
            }
            Err(_) => {
                let count = ctx.lock_fail_count.fetch_add(1, Ordering::Relaxed);
                if count == 0 || count % 100 == 0 {
                    log::error!("DAQ callback: buffer mutex poisoned (count={})", count + 1);
                }
            }
        }
    } else {
        let count = ctx.error_count.fetch_add(1, Ordering::Relaxed);
        if count == 0 || count % 100 == 0 {
            log::error!("DAQ callback: read_analog_f64 failed with code {} (error count={})", code, count + 1);
        }
    }

    0
}

/// Register EveryNSamples callback for continuous acquisition.
pub fn register_callback(
    task: &DaqTask,
    num_channels: usize,
    interval: u32,
    buffer: Arc<Mutex<AccumulationBuffer>>,
) -> DaqmxResult<Box<CallbackContext>> {
    let read_buf_size = num_channels * interval as usize;
    let ctx = Box::new(CallbackContext {
        lib: task.lib as *const DaqmxLib,
        handle: task.handle,
        buffer,
        num_channels,
        read_buf: UnsafeCell::new(vec![0.0f64; read_buf_size]),
        error_count: AtomicU64::new(0),
        lock_fail_count: AtomicU64::new(0),
    });

    let code = unsafe {
        (task.lib.register_every_n_samples_event)(
            task.handle,
            DAQmx_Val_Acquired_Into_Buffer,
            interval,
            0,
            every_n_callback,
            &*ctx as *const CallbackContext as *mut std::ffi::c_void,
        )
    };
    task.lib.check(code)?;

    Ok(ctx)
}
