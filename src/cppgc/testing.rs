// kun: test-only control over cppgc garbage collection, for heaps that are
// not attached to an isolate.
//
// cppgc's `StandaloneTestingHeap` lets a test start an incremental GC, run
// single marking steps, and finalize, so it can mutate the heap at chosen
// points while marking is in progress. V8 only supports it on detached heaps
// (it DCHECKs this), so these functions take a `DetachedHeap`.

use super::{Heap, HeapCreateParams, MarkingType, SweepingType};
use crate::UniqueRef;

unsafe extern "C" {
  fn cppgc__testing__start_gc(heap: *mut Heap);
  fn cppgc__testing__marking_step(heap: *mut Heap) -> bool;
  fn cppgc__testing__finalize_gc(heap: *mut Heap);
  fn cppgc__testing__set_main_thread_marking(heap: *mut Heap, enabled: bool);
  fn cppgc__testing__is_marking(heap: *mut Heap) -> bool;
  fn cppgc__testing__is_sweeping(heap: *mut Heap) -> bool;
}

/// A CppHeap attached to no isolate, the only kind of heap the functions
/// below accept: V8 supports driving GC by hand only on detached heaps.
///
/// It owns the heap and never hands out the `UniqueRef`, which is what
/// attaching a heap to an isolate takes (`CreateParams::cpp_heap`), so a
/// `DetachedHeap` stays detached. Derefs to the heap for allocation.
pub struct DetachedHeap(UniqueRef<Heap>);

impl DetachedHeap {
  /// GC is driven only by the functions below (and
  /// `Heap::collect_garbage_for_testing`). `concurrent` selects
  /// incremental-and-concurrent marking instead of incremental only. The
  /// platform must be initialized (`V8::initialize_platform`).
  pub fn new(concurrent: bool) -> Self {
    let heap = Heap::create(
      crate::V8::get_current_platform(),
      HeapCreateParams {
        marking_support: if concurrent {
          MarkingType::IncrementalAndConcurrent
        } else {
          MarkingType::Incremental
        },
        sweeping_support: SweepingType::IncrementalAndConcurrent,
      },
    );
    heap.enable_detached_garbage_collections_for_testing();
    Self(heap)
  }

  fn raw(&self) -> *mut Heap {
    &*self.0 as *const Heap as *mut Heap
  }
}

impl std::ops::Deref for DetachedHeap {
  type Target = Heap;

  fn deref(&self) -> &Heap {
    &self.0
  }
}

// SAFETY (all functions below): `heap` is a live, detached `v8::CppHeap`
// (that is what `DetachedHeap` wraps and guarantees), used on the thread
// that owns it (`DetachedHeap` isn't `Send`: `UniqueRef<Heap>` isn't).

/// Starts an incremental GC. No-op if one is already marking.
pub fn start_incremental_gc(heap: &DetachedHeap) {
  // SAFETY: see above.
  unsafe { cppgc__testing__start_gc(heap.raw()) }
}

/// Runs one incremental marking step (and reposts concurrent marking jobs).
/// Returns true once marking has no more work. The stack is not scanned, so
/// the caller must not hold unrooted GC pointers on it.
pub fn marking_step(heap: &DetachedHeap) -> bool {
  // SAFETY: see above.
  unsafe { cppgc__testing__marking_step(heap.raw()) }
}

/// Finishes the current GC atomically, including sweeping. The stack is not
/// scanned, as in [`marking_step`].
pub fn finalize_gc(heap: &DetachedHeap) {
  // SAFETY: see above.
  unsafe { cppgc__testing__finalize_gc(heap.raw()) }
}

/// With `false`, the main thread stops contributing to marking, so all of
/// it happens on concurrent marker threads (heap must be concurrent).
///
/// # Panics
///
/// If no GC is marking: cppgc's marker doesn't exist then.
pub fn set_main_thread_marking(heap: &DetachedHeap, enabled: bool) {
  assert!(is_marking(heap), "no GC is marking");
  // SAFETY: see above; a marker exists.
  unsafe { cppgc__testing__set_main_thread_marking(heap.raw(), enabled) }
}

pub fn is_marking(heap: &DetachedHeap) -> bool {
  // SAFETY: see above; only reads heap state.
  unsafe { cppgc__testing__is_marking(heap.raw()) }
}

pub fn is_sweeping(heap: &DetachedHeap) -> bool {
  // SAFETY: see above; only reads heap state.
  unsafe { cppgc__testing__is_sweeping(heap.raw()) }
}
