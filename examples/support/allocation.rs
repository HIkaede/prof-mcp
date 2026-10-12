use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering::Relaxed},
};

pub struct Allocator;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static COUNT: AtomicUsize = AtomicUsize::new(0);

fn add(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Relaxed) + bytes;
    PEAK.fetch_max(live, Relaxed);
    COUNT.fetch_add(1, Relaxed);
}

// System retains responsibility for pointer validity and allocation alignment.
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            add(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = unsafe { System.alloc_zeroed(layout) };
        if !pointer.is_null() {
            add(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
        LIVE.fetch_sub(layout.size(), Relaxed);
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let result = unsafe { System.realloc(pointer, layout, size) };
        if !result.is_null() {
            LIVE.fetch_sub(layout.size(), Relaxed);
            add(size);
        }
        result
    }
}

pub fn begin() -> (usize, usize) {
    let live = LIVE.load(Relaxed);
    PEAK.store(live, Relaxed);
    (live, COUNT.load(Relaxed))
}

pub fn end(start: (usize, usize)) -> (usize, usize, usize) {
    (
        PEAK.load(Relaxed).saturating_sub(start.0),
        COUNT.load(Relaxed) - start.1,
        LIVE.load(Relaxed).saturating_sub(start.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allocation_accounting() {
        let before = begin();
        let mut bytes = Vec::<u8>::with_capacity(1024);
        std::hint::black_box(&bytes);
        // Concurrent harness frees invalidate net growth, but not our absolute live peak.
        assert!(PEAK.load(Relaxed) >= 1024);
        bytes.reserve(4096);
        assert!(PEAK.load(Relaxed) >= bytes.capacity());
        drop(bytes);
        assert!(end(before).1 >= 2);
    }
}
