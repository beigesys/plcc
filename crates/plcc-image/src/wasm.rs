// SPDX-License-Identifier: MPL-2.0

//! A plain C ABI for the WebAssembly build (`--features wasm-abi`), used by
//! `packages/plc-image`: no wasm-bindgen, so the package needs only
//! `WebAssembly.instantiate`. Memory protocol: the host allocates input
//! buffers with `plcc_image_alloc`, calls `plcc_image_link`, then reads the
//! image (`plcc_image_output_ptr/len`) and a JSON report
//! (`plcc_image_report_ptr/len`), which on failure holds `{"ok":false,"error":…}`.

use crate::json::{json_str, report_json};
use crate::{Layout, Options, link};
use std::sync::Mutex;

struct Out {
    image: Vec<u8>,
    report: Vec<u8>,
}

static OUT: Mutex<Out> = Mutex::new(Out { image: Vec::new(), report: Vec::new() });

#[unsafe(no_mangle)]
pub extern "C" fn plcc_image_alloc(len: usize) -> *mut u8 {
    let mut v = vec![0u8; len.max(1)];
    let p = v.as_mut_ptr();
    std::mem::forget(v);
    p
}

/// # Safety
/// `ptr`/`len` must come from `plcc_image_alloc(len)`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn plcc_image_free(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        drop(unsafe { Vec::from_raw_parts(ptr, len.max(1), len.max(1)) });
    }
}

/// Link the object at `obj`. `id` is the device id (UTF-8), `build_id` null
/// or 16 bytes. Returns 0 on success, 1 on a link error (see the report).
///
/// # Safety
/// The pointers must be valid for their lengths.
#[unsafe(no_mangle)]
#[allow(clippy::too_many_arguments)]
pub unsafe extern "C" fn plcc_image_link(
    obj: *const u8,
    obj_len: usize,
    id: *const u8,
    id_len: usize,
    target_version: u32,
    abi: u32,
    slot_addr: u32,
    slot_size: u32,
    ram_addr: u32,
    ram_size: u32,
    services: u32,
    hard_float: u32,
    build_id: *const u8,
) -> u32 {
    let obj = unsafe { std::slice::from_raw_parts(obj, obj_len) };
    let id = String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(id, id_len) }).into_owned();
    let build_id = if build_id.is_null() {
        None
    } else {
        let mut b = [0u8; 16];
        b.copy_from_slice(unsafe { std::slice::from_raw_parts(build_id, 16) });
        Some(b)
    };
    let layout = Layout {
        target_id: id,
        target_version,
        abi,
        slot_addr,
        slot_size,
        ram_addr,
        ram_size,
        services,
        hard_float: hard_float != 0,
    };
    let mut out = OUT.lock().unwrap_or_else(|e| e.into_inner());
    match link(obj, &layout, &Options { build_id }) {
        Ok(img) => {
            out.report = report_json(&img).into_bytes();
            out.image = img.bytes;
            0
        }
        Err(e) => {
            out.report = format!("{{\"ok\":false,\"error\":{}}}", json_str(&e.message)).into_bytes();
            out.image = Vec::new();
            1
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn plcc_image_output_ptr() -> *const u8 {
    OUT.lock().map(|o| o.image.as_ptr()).unwrap_or(std::ptr::null())
}

#[unsafe(no_mangle)]
pub extern "C" fn plcc_image_output_len() -> usize {
    OUT.lock().map(|o| o.image.len()).unwrap_or(0)
}

#[unsafe(no_mangle)]
pub extern "C" fn plcc_image_report_ptr() -> *const u8 {
    OUT.lock().map(|o| o.report.as_ptr()).unwrap_or(std::ptr::null())
}

#[unsafe(no_mangle)]
pub extern "C" fn plcc_image_report_len() -> usize {
    OUT.lock().map(|o| o.report.len()).unwrap_or(0)
}
