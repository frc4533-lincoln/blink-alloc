//! This module provides multi-threaded blink allocator\
//! with sync resets.

use core::{alloc::Layout, mem::ManuallyDrop, ptr::NonNull};

#[cfg(not(feature = "nightly"))]
use allocator_api2::alloc::{AllocError, Allocator, Global};
#[cfg(feature = "nightly")]
use std::alloc::{AllocError, Allocator, Global};

use crate::{api::BlinkAllocator, arena::ArenaLocal};

switch_alloc_default! {
    /// Single-threaded blink allocator.
    ///
    /// Blink-allocator is arena-based allocator that
    /// allocates memory in growing chunks and serve allocations from them.
    /// When chunk is exhausted a new larger chunk is allocated.
    ///
    /// Deallocation is no-op. [`BlinkAlloc`] can be reset
    /// to free all chunks except the last one, that will be reused.
    ///
    /// Blink allocator aims to allocate a chunk large enough to
    /// serve all allocations between resets.
    ///
    /// A shared and mutable reference to the [`BlinkAlloc`] implement
    /// [`Allocator`] trait.
    /// When "nightly" feature is enabled, [`Allocator`] trait is
    /// [`core::alloc::Allocator`]. Otherwise it is duplicated trait defined
    /// in [`allocator-api2`](allocator_api2).
    ///
    /// Resetting blink allocator requires mutable borrow, so it is not possible
    /// to do while shared borrow is alive. That matches requirement of
    /// [`Allocator`] trait - while [`Allocator`] instance
    /// (a shared reference to [`BlinkAlloc`]) or any of its clones are alive,
    /// allocated memory must be valid.
    ///
    /// This version of blink-allocator is single-threaded. It is possible
    /// to send to another thread, but cannot be shared.
    /// Internally it uses [`Cell`](core::cell::Cell) for interior mutability and requires
    /// that state cannot be changed from another thread.
    ///
    #[cfg_attr(feature = "sync", doc = "For multi-threaded version see [`SyncBlinkAlloc`](crate::sync::SyncBlinkAlloc).")]
    #[cfg_attr(not(feature = "sync"), doc = "For multi-threaded version see `SyncBlinkAlloc`.")]
    /// Requires `"sync"` feature.
    ///
    /// # Example
    ///
    /// ```
    /// # #![cfg_attr(feature = "nightly", feature(allocator_api))]
    /// # #[cfg(not(feature = "alloc"))] fn main() {}
    /// # #[cfg(feature = "alloc")] fn main() {
    /// # use blink_alloc::BlinkAlloc;
    /// # use std::ptr::NonNull;
    ///
    /// let mut blink = BlinkAlloc::new();
    /// let layout = std::alloc::Layout::new::<[u32; 8]>();
    /// let ptr = blink.allocate(layout).unwrap();
    /// let ptr = NonNull::new(ptr.as_ptr() as *mut u8).unwrap(); // Method for this is unstable.
    ///
    /// unsafe {
    ///     std::ptr::write(ptr.as_ptr().cast(), [1, 2, 3, 4, 5, 6, 7, 8]);
    /// }
    ///
    /// blink.reset();
    /// # }
    /// ```
    ///
    /// # Example that uses nightly's `allocator_api`
    ///
    /// ```
    /// # #![cfg_attr(feature = "nightly", feature(allocator_api))]
    /// # #[cfg(feature = "alloc")]
    /// # fn main() {
    /// # use blink_alloc::BlinkAlloc;
    /// # use allocator_api2::vec::Vec;
    /// let mut blink = BlinkAlloc::new();
    /// let mut vec = Vec::new_in(&blink);
    /// vec.push(1);
    /// vec.extend(1..3);
    /// vec.extend(3..10);
    /// drop(vec);
    /// blink.reset();
    /// # }
    /// # #[cfg(not(feature = "alloc"))] fn main() {}
    /// ```
    pub struct BlinkAlloc<A: Allocator = +Global> {
        arena: ArenaLocal,
        allocator: A,
    }
}

impl<A> Drop for BlinkAlloc<A>
where
    A: Allocator,
{
    #[inline]
    fn drop(&mut self) {
        // Safety:
        // Same instance is used for all allocations and resets.
        unsafe {
            self.arena.reset(false, &self.allocator);
        }
    }
}

impl<A> Default for BlinkAlloc<A>
where
    A: Allocator + Default,
{
    #[inline]
    fn default() -> Self {
        Self::new_in(Default::default())
    }
}

#[cfg(feature = "alloc")]
impl BlinkAlloc<Global> {
    /// Creates new blink allocator that uses global allocator
    /// to allocate memory chunks.
    ///
    /// See [`BlinkAlloc::new_in`] for using custom allocator.
    #[inline]
    pub const fn new() -> Self {
        BlinkAlloc::new_in(Global)
    }

    /// Creates new blink allocator that uses global allocator
    /// to allocate memory chunks.
    /// With this method you can specify initial chunk size.
    ///
    /// See [`BlinkAlloc::new_in`] for using custom allocator.
    #[inline]
    pub const fn with_chunk_size(chunk_size: usize) -> Self {
        BlinkAlloc::with_chunk_size_in(chunk_size, Global)
    }
}

impl<A> BlinkAlloc<A>
where
    A: Allocator,
{
    /// Creates new blink allocator that uses provided allocator
    /// to allocate memory chunks.
    ///
    /// See [`BlinkAlloc::new`] for using global allocator.
    #[inline]
    pub const fn new_in(allocator: A) -> Self {
        BlinkAlloc {
            arena: ArenaLocal::new(),
            allocator,
        }
    }

    /// Returns reference to the underlying allocator used by this blink allocator.
    #[inline(always)]
    pub const fn inner(&self) -> &A {
        &self.allocator
    }

    /// Creates new blink allocator that uses global allocator
    /// to allocate memory chunks.
    /// With this method you can specify initial chunk size.
    ///
    /// See [`BlinkAlloc::new_in`] for using custom allocator.
    #[inline]
    pub const fn with_chunk_size_in(chunk_size: usize, allocator: A) -> Self {
        BlinkAlloc {
            arena: ArenaLocal::with_chunk_size(chunk_size),
            allocator,
        }
    }

    /// Allocates memory with specified layout from this allocator.
    /// If needed it will allocate new chunk using underlying allocator.
    /// If chunk allocation fails, it will return `Err`.
    #[inline(always)]
    pub fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        // Safety:
        // Same instance is used for all allocations and resets.
        if let Some(ptr) = unsafe { self.arena.alloc_fast(layout) } {
            return Ok(ptr);
        }
        unsafe { self.arena.alloc_slow(layout, &self.allocator) }
    }

    /// Resizes memory allocation.
    /// Potentially happens in-place.
    ///
    /// # Safety
    ///
    /// `ptr` must be a pointer previously returned by [`allocate`](BlinkAlloc::allocate).
    /// `old_size` must be in range `layout.size()..=slice.len()`
    /// where `layout` is the layout used in the call to [`allocate`](BlinkAlloc::allocate).
    /// and `slice` is the slice pointer returned by [`allocate`](BlinkAlloc::allocate).
    ///
    /// On success, the old pointer is invalidated and the new pointer is returned.
    /// On error old allocation is still valid.
    #[inline(always)]
    pub unsafe fn resize(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> {
        if let Some(ptr) = unsafe { self.arena.resize_fast(ptr, old_layout, new_layout) } {
            return Ok(ptr);
        }

        // Safety:
        // Same instance is used for all allocations and resets.
        // `ptr` was allocated by this allocator.
        unsafe {
            self.arena
                .resize_slow(ptr, old_layout, new_layout, &self.allocator)
        }
    }

    /// Deallocates memory previously allocated from this allocator.
    ///
    /// This call may not actually free memory.
    /// All memory is guaranteed to be freed on [`reset`](BlinkAlloc::reset) call.
    ///
    /// # Safety
    ///
    /// `ptr` must be a pointer previously returned by [`allocate`](BlinkAlloc::allocate).
    /// `size` must be in range `layout.size()..=slice.len()`
    /// where `layout` is the layout used in the call to [`allocate`](BlinkAlloc::allocate).
    /// and `slice` is the slice pointer returned by [`allocate`](BlinkAlloc::allocate).
    #[inline(always)]
    pub unsafe fn deallocate(&self, ptr: NonNull<u8>, size: usize) {
        // Safety:
        // `ptr` was allocated by this allocator.
        unsafe {
            self.arena.dealloc(ptr, size);
        }
    }

    /// Resets this allocator, deallocating all chunks except the last one.
    /// Last chunk will be reused.
    /// With steady memory usage after few iterations
    /// one chunk should be sufficient for all allocations between resets.
    #[inline(always)]
    pub fn reset(&mut self) {
        // Safety:
        // Same instance is used for all allocations and resets.
        unsafe {
            self.arena.reset(true, &self.allocator);
        }
    }

    /// Resets this allocator, deallocating all chunks.
    #[inline(always)]
    pub fn reset_final(&mut self) {
        // Safety:
        // Same instance is used for all allocations and resets.
        unsafe {
            self.arena.reset(false, &self.allocator);
        }
    }

    /// Resets this allocator, deallocating all chunks except the last one.
    /// Last chunk will be reused.
    /// With steady memory usage after few iterations
    /// one chunk should be sufficient for all allocations between resets.
    ///
    /// # Safety
    ///
    /// Blink-allocators guarantee that memory can be used while shared
    /// borrow to the allocator is held, preventing safe `fn reset` call.
    ///
    /// With this method it becomes caller responsibility to ensure
    /// that allocated memory won't be used after reset.
    #[inline(always)]
    pub unsafe fn reset_unchecked(&self) {
        // Safety:
        // Same instance is used for all allocations and resets.
        unsafe {
            self.arena.reset_unchecked(true, &self.allocator);
        }
    }

    /// Unwrap this allocator, returning the underlying allocator.
    /// Leaks allocated chunks.
    ///
    /// To deallocate all chunks call [`reset_final`](BlinkAlloc::reset_final) first.
    pub fn into_inner(self) -> A {
        let me = ManuallyDrop::new(self);
        unsafe { core::ptr::read(&me.allocator) }
    }
}

unsafe impl<A> Allocator for BlinkAlloc<A>
where
    A: Allocator,
{
    #[inline(always)]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        BlinkAlloc::allocate(self, layout)
    }

    #[inline(always)]
    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> { unsafe {
        BlinkAlloc::resize(self, ptr, old_layout, new_layout)
    }}

    #[inline(always)]
    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> { unsafe {
        BlinkAlloc::resize(self, ptr, old_layout, new_layout)
    }}

    #[inline(always)]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) { unsafe {
        BlinkAlloc::deallocate(self, ptr, layout.size());
    }}
}

unsafe impl<A> Allocator for &mut BlinkAlloc<A>
where
    A: Allocator,
{
    #[inline(always)]
    fn allocate(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        BlinkAlloc::allocate(self, layout)
    }

    #[inline(always)]
    fn allocate_zeroed(&self, layout: Layout) -> Result<NonNull<[u8]>, AllocError> {
        BlinkAlloc::allocate_zeroed(self, layout)
    }

    #[inline(always)]
    unsafe fn shrink(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> { unsafe {
        BlinkAlloc::resize(self, ptr, old_layout, new_layout)
    }}

    #[inline(always)]
    unsafe fn grow(
        &self,
        ptr: NonNull<u8>,
        old_layout: Layout,
        new_layout: Layout,
    ) -> Result<NonNull<[u8]>, AllocError> { unsafe {
        BlinkAlloc::resize(self, ptr, old_layout, new_layout)
    }}

    #[inline(always)]
    unsafe fn deallocate(&self, ptr: NonNull<u8>, layout: Layout) { unsafe {
        BlinkAlloc::deallocate(self, ptr, layout.size());
    }}
}

unsafe impl<A> BlinkAllocator for BlinkAlloc<A>
where
    A: Allocator,
{
    #[inline(always)]
    fn reset(&mut self) {
        BlinkAlloc::reset(self)
    }
}
