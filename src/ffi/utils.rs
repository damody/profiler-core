use widestring::U16CString;

/// Allocate a null-terminated UTF-16 (wide) string on the heap and return a
/// raw pointer.  The caller (C# side, or `free_wide_ptr`) is responsible for
/// freeing the memory.
pub fn to_wide_ptr(s: &str) -> *mut u16 {
    let wide = U16CString::from_str(s).unwrap_or_else(|_| U16CString::default());
    // into_raw() transfers ownership of the inner Vec's buffer to us.
    wide.into_raw() as *mut u16
}

/// Read a null-terminated UTF-16 string from a raw pointer.
///
/// # Safety
/// The pointer must be valid and point to a null-terminated UTF-16 string.
pub unsafe fn from_wide_ptr(ptr: *const u16) -> String {
    if ptr.is_null() {
        return String::new();
    }
    let wide = unsafe { U16CString::from_ptr_str(ptr) };
    wide.to_string_lossy()
}

/// Free a wide string previously allocated by `to_wide_ptr`.
///
/// # Safety
/// The pointer must have been allocated by `to_wide_ptr` (i.e. by
/// `U16CString::into_raw`).
pub unsafe fn free_wide_ptr(ptr: *mut u16) {
    if !ptr.is_null() {
        drop(unsafe { U16CString::from_raw(ptr) });
    }
}
