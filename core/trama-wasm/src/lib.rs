// SPDX-License-Identifier: LicenseRef-BSL-1.1
//! Compiling in a browser, through the same code the command line runs.
//!
//! No bindgen: three C-ABI functions and a shared linear memory are enough for "bytes in,
//! bytes out", and they keep the module small enough to be worth loading at all.
//!
//! The caller writes the source JSON into memory obtained from `trama_alloc`, calls
//! `trama_compile`, reads the result, and frees both with `trama_free`.

use std::alloc::{Layout, alloc, dealloc};

use serde_json::Value;

/// Reserve `bytes` for the caller to write into. Freed with `trama_free`.
#[unsafe(no_mangle)]
pub extern "C" fn trama_alloc(bytes: usize) -> *mut u8 {
    if bytes == 0 {
        return std::ptr::null_mut();
    }
    unsafe { alloc(Layout::from_size_align(bytes, 1).unwrap()) }
}

/// Release memory from `trama_alloc` or `trama_compile`.
///
/// # Safety
/// `pointer` must come from this module and `bytes` must be the length it was given.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn trama_free(pointer: *mut u8, bytes: usize) {
    if !pointer.is_null() && bytes > 0 {
        unsafe { dealloc(pointer, Layout::from_size_align(bytes, 1).unwrap()) };
    }
}

/// Compile a GeoJSON FeatureCollection. Writes the container's length into `out_length` and
/// returns its start, or null on failure, in which case `out_length` holds the message length
/// and `trama_error` returns the message.
///
/// # Safety
/// `source` must point at `length` readable bytes and `out_length` at a writable `usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn trama_compile(source: *const u8, length: usize, out_length: *mut usize) -> *mut u8 {
    let text = unsafe { std::slice::from_raw_parts(source, length) };
    let outcome = serde_json::from_slice::<Value>(text).map_err(|error| error.to_string()).and_then(|parsed| {
        let features = parsed["features"].as_array().cloned().unwrap_or_default();
        trama_format::compile(&features, &[], &[])
    });
    match outcome {
        Ok(bytes) => unsafe { release(bytes, out_length) },
        Err(message) => {
            LAST_ERROR.with(|held| *held.borrow_mut() = message);
            unsafe { *out_length = LAST_ERROR.with(|held| held.borrow().len()) };
            std::ptr::null_mut()
        }
    }
}

/// The message behind the last failure, as bytes the caller must free.
///
/// # Safety
/// `out_length` must point at a writable `usize`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn trama_error(out_length: *mut usize) -> *mut u8 {
    let message = LAST_ERROR.with(|held| held.borrow().clone());
    unsafe { release(message.into_bytes(), out_length) }
}

thread_local! {
    static LAST_ERROR: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

unsafe fn release(mut bytes: Vec<u8>, out_length: *mut usize) -> *mut u8 {
    bytes.shrink_to_fit();
    unsafe { *out_length = bytes.len() };
    let pointer = bytes.as_mut_ptr();
    std::mem::forget(bytes);
    pointer
}
