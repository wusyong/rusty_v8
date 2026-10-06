// kun: a cppgc heap with no isolate, for an embedder that runs no V8 (kun's
// wasm engine).
//
// A `Heap` (`v8::CppHeap`) never collects by itself while no isolate is
// attached (`CppHeap::IsGCAllowed`). A standalone `cppgc::Heap` does: by
// allocation it starts incremental marking, and it runs the marking steps,
// the final pause and incremental sweeping as non-nestable foreground
// tasks. The heap is created with `StackSupport::kNoConservativeStackScan`,
// so cppgc never scans the stack: it collects only in those tasks, which
// the embedder runs with nothing on the stack. The tasks reach the
// embedder through a `PlatformImpl`, as a `Platform::new_custom` platform's
// do, with a null isolate pointer; worker jobs (concurrent marking and
// sweeping) run on the `Platform`'s threads.
//
// Objects are allocated with `make_garbage_collected_on` on the heap's
// `allocation_handle`. `TracedReference`s must stay empty on this heap:
// tracing one casts cppgc's visitor to V8's `JSVisitor`, which only a
// `CppHeap`'s is (an empty one returns before using the visitor).

use super::{AllocationHandle, EmbedderStackState, HeapCreateParams};
use super::{MarkingType, SweepingType};
use crate::platform::{Platform, PlatformImpl};
use crate::support::{Opaque, SharedRef};
use std::ffi::c_void;
use std::ptr::NonNull;

unsafe extern "C" {
  fn cppgc__StandaloneHeap__Create(
    platform: *mut Platform,
    context: *mut c_void,
    marking_support: MarkingType,
    sweeping_support: SweepingType,
  ) -> *mut RawHeap;
  fn cppgc__StandaloneHeap__DELETE(heap: *mut RawHeap);
  fn cppgc__StandaloneHeap__GetAllocationHandle(
    heap: *mut RawHeap,
  ) -> *mut AllocationHandle;
  fn cppgc__StandaloneHeap__ForceGarbageCollectionSlow(
    heap: *mut RawHeap,
    stack_state: EmbedderStackState,
  );
}

/// `cppgc::Heap`.
#[repr(C)]
struct RawHeap(Opaque);

/// A standalone cppgc heap: attached to no isolate, it schedules its own
/// GCs through the foreground tasks it posts (see the module's comment).
///
/// Used from one thread, the one that runs its tasks (it isn't `Send`).
pub struct StandaloneHeap {
  heap: NonNull<RawHeap>,
  // The C++ platform the heap owns points into it.
  _platform: SharedRef<Platform>,
}

impl StandaloneHeap {
  /// `cppgc::initialize_process` must have been called. `tasks` receives
  /// the heap's foreground tasks (with a null isolate pointer) and must run
  /// each on this heap's thread, after it has been posted (and after its
  /// delay), outside any trace or finalizer, and with no unrooted GC
  /// pointer on the stack: cppgc marks the stack as empty when it runs
  /// them. Tasks left in the queue when the heap is dropped do nothing.
  pub fn new(
    platform: SharedRef<Platform>,
    params: HeapCreateParams,
    tasks: impl PlatformImpl + 'static,
  ) -> Self {
    // As `Platform::new_custom`: the outer box is a thin pointer to pass
    // as C++'s `void*`, which `v8__Platform__CustomPlatform__BASE__DROP`
    // frees when the heap destroys its platform.
    let tasks: Box<dyn PlatformImpl> = Box::new(tasks);
    let context = Box::into_raw(Box::new(tasks)) as *mut c_void;
    // SAFETY: `platform` is alive and stays so (kept in `_platform`);
    // `context` is the double box the callbacks expect, owned by the heap
    // from here on.
    let heap = unsafe {
      cppgc__StandaloneHeap__Create(
        &*platform as *const Platform as *mut _,
        context,
        params.marking_support,
        params.sweeping_support,
      )
    };
    Self {
      heap: NonNull::new(heap).unwrap(),
      _platform: platform,
    }
  }

  /// Where [`super::make_garbage_collected_on`] allocates on this heap.
  pub fn allocation_handle(&self) -> &AllocationHandle {
    // SAFETY: the heap is alive; the handle lives as long as it does.
    unsafe { &*cppgc__StandaloneHeap__GetAllocationHandle(self.heap.as_ptr()) }
  }

  /// A full atomic GC now, sweeping included; finishes one that is marking.
  ///
  /// # Safety
  ///
  /// No unrooted GC pointer is on the stack (the stack isn't scanned), and
  /// this isn't called from a trace or a finalizer.
  pub unsafe fn collect_garbage(&self) {
    // SAFETY: the heap is alive; the stack is the caller's to vouch for.
    unsafe {
      cppgc__StandaloneHeap__ForceGarbageCollectionSlow(
        self.heap.as_ptr(),
        EmbedderStackState::NoHeapPointers,
      )
    }
  }
}

impl Drop for StandaloneHeap {
  /// Runs a precise GC (finishing one that is marking), so every object
  /// left on the heap is finalized, then destroys the heap. `cppgc::Heap`'s
  /// destructor alone would finalize nothing. Every `Persistent` into the
  /// heap must be gone, so that GC finds nothing alive; the caller vouches
  /// for the stack, as for [`collect_garbage`](Self::collect_garbage).
  fn drop(&mut self) {
    // SAFETY: the heap is alive and dropped once; `_platform` outlives it.
    // No unrooted GC pointer is on the stack: dropping the heap ends any
    // use of it.
    unsafe { cppgc__StandaloneHeap__DELETE(self.heap.as_ptr()) }
  }
}
