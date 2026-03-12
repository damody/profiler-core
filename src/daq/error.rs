use thiserror::Error;

#[derive(Error, Debug)]
pub enum DaqmxError {
    #[error("DAQmx library not found: {0}")]
    LibraryNotFound(String),
    #[error("DAQmx error (code {code}): {message}")]
    DaqmxCall { code: i32, message: String },
    #[error("No DAQmx devices found")]
    NoDevices,
}

pub type DaqmxResult<T> = Result<T, DaqmxError>;

/// Check DAQmx return code. code < 0 is error, > 0 is warning, 0 is success.
pub fn check_error(code: i32, get_error_string: &dyn Fn() -> String) -> DaqmxResult<()> {
    if code < 0 {
        let message = get_error_string();
        Err(DaqmxError::DaqmxCall { code, message })
    } else if code > 0 {
        let message = get_error_string();
        log::warn!("DAQmx warning ({}): {}", code, message);
        Ok(())
    } else {
        Ok(())
    }
}
