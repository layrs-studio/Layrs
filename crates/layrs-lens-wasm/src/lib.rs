//! Built-in Lens guest: no WASI and no imports. Each invocation receives a
//! fresh bounded linear memory and can only return a canonical byte buffer.
#![forbid(unsafe_op_in_unsafe_fn)]
#![cfg(target_arch = "wasm32")]

#[unsafe(no_mangle)]
pub extern "C" fn layrs_allocate(length: u32) -> u32 {
    if length == 0 || length > 16 * 1024 * 1024 {
        return 0;
    }
    Box::into_raw(vec![0u8; length as usize].into_boxed_slice()) as *mut u8 as usize as u32
}

/// # Safety
/// Host must pass the exact initialized allocation returned by layrs_allocate.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn layrs_reconcile(pointer: u32, length: u32) -> u64 {
    if pointer == 0 || length == 0 || length > 16 * 1024 * 1024 {
        return 0;
    }
    let input =
        unsafe { std::slice::from_raw_parts(pointer as usize as *const u8, length as usize) };
    let mut output = Vec::new();
    if layrs_weave::worker::serve(input, &mut output).is_err() || output.len() > 16 * 1024 * 1024 {
        return 0;
    }
    let length = output.len() as u64;
    let pointer = Box::into_raw(output.into_boxed_slice()) as *mut u8 as usize as u64;
    (pointer << 32) | length
}
