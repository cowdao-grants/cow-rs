//! Compact global allocator for the wasm32 target.
//!
//! Replaces dlmalloc (~10 KB) with `lol_alloc`'s single-threaded
//! free-list allocator (~5 KB). The wasm shim is single-threaded by
//! construction — wasm-bindgen entry points run on the main JS thread
//! — so the `AssumeSingleThreaded` wrapper drops the lock the
//! free-list would otherwise need.
//!
//! Native host builds keep dlmalloc; the `#[cfg]` gate below makes
//! this module a no-op outside wasm32.

#[cfg(target_arch = "wasm32")]
#[global_allocator]
#[allow(unsafe_code)]
static ALLOC: lol_alloc::AssumeSingleThreaded<lol_alloc::FreeListAllocator> =
    unsafe { lol_alloc::AssumeSingleThreaded::new(lol_alloc::FreeListAllocator::new()) };
