//! kun: an embedder heap of kun's own (not cppgc's) marking with V8's major
//! GCs as cppgc's does in Chrome's unified heap, through kun's patch to V8
//! (`v8/include/v8-kun.h`, in kun's V8 fork) and the glue's C functions for
//! it (`src/binding.cc`). Thin: the embedder's heap and its rules are kun's
//! `bindings`'; this module only gives V8's side Rust types.
//!
//! - **Marking.** V8 starts the embedder heap's marking with its own,
//!   advances it with each of its steps, asks whether it is done before it
//!   finishes, and ends it once both are ([`EmbedderMarking`],
//!   [`set_embedder_marking`]). The heap's used bytes count with cppgc's
//!   ([`report_embedder_used_size`]).
//! - **Wrappers.** A wrapper holds its object's address with a tag of
//!   [`FIRST_TAG`]`..=`[`LAST_TAG`] (one per type; [`wrap`], [`unwrap`]):
//!   V8's marking threads hand the object to the embedder heap's marking as
//!   they mark the wrapper, and its write barrier, young generation and
//!   heap snapshot leave it alone. The objects never move.
//! - **References to wrappers.** An object refers to its wrapper with a
//!   droppable `TracedReference` in a [`TracedSlot`] in the object
//!   ([`TracedSlot::set_droppable`]): the embedder heap's marking threads
//!   mark it into V8's worklists ([`TracedMarker`]); V8's scavenge may drop
//!   an unmodified wrapper nothing else holds, and asks the embedder to
//!   reset the reference ([`RootsHandler`]).
//! - **Workers.** The embedder heap's parallel work runs on V8's platform
//!   ([`post_job`]), sharing its workers, as cppgc's does.
//!
//! Callbacks from C++ abort the process on a panic: it can't unwind there.
//!
//! Only with pointer compression: the patch tells kun's objects from
//! cppgc's by their entries' tags in the `CppHeapPointerTable`.

use std::ffi::c_void;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{self, NonNull};

use crate::cppgc::{Heap, TracedSlot};
use crate::isolate::RealIsolate;
use crate::scope::GetIsolate;
use crate::support::Opaque;
use crate::{
  Data, Isolate, Local, Object, PinScope, Platform, TracedReference,
  UnsafeRawIsolatePtr,
};

/// The first of the tags of wrappers holding an embedder heap's object
/// (`v8::kun::kFirstTag`).
pub const FIRST_TAG: u16 = 0x4000;
/// The last of them (`v8::kun::kLastTag`).
pub const LAST_TAG: u16 = 0x4fff;

/// `v8::kun::EmbedderMarking`'s methods, which the glue's
/// `RustEmbedderMarking` calls with its data (`RustEmbedderMarking::Fns`;
/// copied as it is made).
#[repr(C)]
struct MarkingFns {
  start: unsafe extern "C" fn(*mut c_void),
  advance: unsafe extern "C" fn(*mut c_void, f64, bool) -> bool,
  is_done: unsafe extern "C" fn(*mut c_void) -> bool,
  end: unsafe extern "C" fn(*mut c_void) -> usize,
  new_local: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
  hand: unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void),
  publish: unsafe extern "C" fn(*mut c_void, *mut c_void),
  is_local_empty: unsafe extern "C" fn(*mut c_void, *mut c_void) -> bool,
  delete_local: unsafe extern "C" fn(*mut c_void, *mut c_void),
}

/// `RustJobTask::Fns` in the glue (copied as the task is made).
#[repr(C)]
struct JobFns {
  run: unsafe extern "C" fn(*mut c_void, *mut JobDelegate),
  max_concurrency: unsafe extern "C" fn(*mut c_void, usize) -> usize,
  drop: unsafe extern "C" fn(*mut c_void),
}

type ResetRootFn =
  unsafe extern "C" fn(*mut c_void, *mut c_void, *const RootHandle);
type TryResetRootFn =
  unsafe extern "C" fn(*mut c_void, *mut c_void, *const RootHandle) -> bool;

unsafe extern "C" {
  fn v8__kun__EmbedderMarking__NEW(
    data: *mut c_void,
    fns: *const MarkingFns,
  ) -> *mut c_void;
  fn v8__kun__EmbedderMarking__DELETE(marking: *mut c_void);
  fn v8__kun__FinishMarking(isolate: *mut RealIsolate);
  fn v8__kun__SetEmbedderMarking(heap: *const Heap, marking: *mut c_void);
  fn v8__kun__ReportEmbedderUsedSize(isolate: *mut RealIsolate, used: usize);
  fn v8__kun__Wrap(
    isolate: *mut RealIsolate,
    wrapper: *const Object,
    object: *mut c_void,
    tag: u16,
  );
  fn v8__kun__Object__Clone(
    isolate: *mut RealIsolate,
    object: *const Object,
  ) -> *const Object;
  fn v8__kun__Unwrap(
    isolate: *mut RealIsolate,
    wrapper: *const Object,
    tag: u16,
  ) -> *mut c_void;
  fn v8__kun__TracedReference__SetDroppable(
    isolate: *mut RealIsolate,
    slot: *mut TracedReference<Data>,
    value: *const Object,
  );
  fn v8__kun__TracedReference__ResetIfSame(
    slot: *mut TracedReference<Data>,
    handle: *const RootHandle,
  ) -> bool;
  fn v8__kun__TracedMarker__NEW(isolate: *mut RealIsolate) -> *mut c_void;
  fn v8__kun__TracedMarker__DELETE(marker: *mut c_void);
  fn v8__kun__TracedMarker__Mark(
    marker: *mut c_void,
    reference: *const TracedReference<Data>,
  );
  fn v8__kun__TracedMarker__Publish(marker: *mut c_void);
  fn v8__kun__Platform__PostJob(
    platform: *const Platform,
    user_blocking: bool,
    data: *mut c_void,
    fns: *const JobFns,
  ) -> *mut c_void;
  fn v8__kun__JobHandle__NotifyConcurrencyIncrease(handle: *mut c_void);
  fn v8__kun__JobHandle__Cancel(handle: *mut c_void);
  fn v8__kun__JobHandle__CancelAndDetach(handle: *mut c_void);
  fn v8__kun__JobDelegate__ShouldYield(delegate: *mut JobDelegate) -> bool;
  fn v8__kun__JobDelegate__NotifyConcurrencyIncrease(
    delegate: *mut JobDelegate,
  );
  fn v8__kun__RootsHandler__NEW(
    isolate: *mut RealIsolate,
    data: *mut c_void,
    reset_root: ResetRootFn,
    try_reset_root: TryResetRootFn,
  ) -> *mut c_void;
  fn v8__kun__RootsHandler__DELETE(
    isolate: *mut RealIsolate,
    handler: *mut c_void,
  );
}

/// Runs `f` for a call from C++: a panic aborts (it can't unwind there).
fn from_cxx<R>(f: impl FnOnce() -> R) -> R {
  catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| std::process::abort())
}

// --- Marking -----------------------------------------------------------------

/// The embedder heap's side of V8's major GCs (`v8::kun::EmbedderMarking`).
/// V8 calls `start`, `advance`, `is_done` and `end` on the isolate's
/// thread; the locals' functions on its marking threads, each local on one
/// thread at a time (the marking state holding it).
pub trait EmbedderMarking: Sync + 'static {
  /// A marking thread's list of the objects it hands over.
  type Local: Send + 'static;

  /// A major GC's marking starts.
  fn start(&self);

  /// Marks for up to `max_duration_ms`, what V8's step leaves after
  /// cppgc's (all that is left `in_atomic_pause`): whether nothing is left.
  fn advance(&self, max_duration_ms: f64, in_atomic_pause: bool) -> bool;

  /// Whether nothing is left now (V8's locals published).
  fn is_done(&self) -> bool;

  /// Both sides are done, in the pause, after V8 cleared what died: the
  /// embedder heap's used bytes after the marking, its live ones (what it
  /// marked, and what it allocated meanwhile), which V8 counts as its size
  /// from now on and sets its next limits from.
  fn end(&self) -> usize;

  /// A marking thread's local, as V8 makes its marking state; none if the
  /// heap doesn't mark now.
  fn new_local(&self) -> Option<Self::Local>;

  /// V8 marked a wrapper holding `object` with one of the tags
  /// ([`wrap`]'s address).
  fn hand(local: &mut Self::Local, object: NonNull<c_void>);

  /// The objects handed over on this thread, for the heap's marking.
  fn publish(local: &mut Self::Local);

  fn is_local_empty(local: &Self::Local) -> bool;
}

/// An embedder marking set on an isolate's `CppHeap`
/// ([`set_embedder_marking`]): unset when dropped, after finishing a major
/// GC that runs (with the marking: it ends as in any GC), as Chrome
/// finishes its GC before it detaches a `CppHeap`. No V8 thread calls it
/// after.
pub struct EmbedderMarkingRegistration {
  isolate: *mut RealIsolate,
  heap: *const Heap,
  glue: *mut c_void,
  marking: *mut c_void,
  drop_marking: unsafe fn(*mut c_void),
}

/// Sets `marking` as the embedder heap's marking of `isolate`'s major GCs,
/// from the next one on.
///
/// # Safety
///
/// The registration is dropped on the isolate's thread, before the isolate
/// is.
///
/// # Panics
///
/// If the isolate has no `CppHeap`.
pub unsafe fn set_embedder_marking<M: EmbedderMarking>(
  isolate: &mut Isolate,
  marking: Box<M>,
) -> EmbedderMarkingRegistration {
  let heap = ptr::from_ref(
    isolate
      .get_cpp_heap()
      .expect("the isolate was created with a CppHeap"),
  );
  let marking = Box::into_raw(marking);
  let fns = MarkingFns {
    start: start::<M>,
    advance: advance::<M>,
    is_done: is_done::<M>,
    end: end::<M>,
    new_local: new_local::<M>,
    hand: hand::<M>,
    publish: publish::<M>,
    is_local_empty: is_local_empty::<M>,
    delete_local: delete_local::<M>,
  };
  /// Drops the `Box<M>` at `marking`.
  unsafe fn drop_box<M>(marking: *mut c_void) {
    // SAFETY: as the caller promises: `Box::into_raw` of a `Box<M>`,
    // dropped once.
    drop(unsafe { Box::from_raw(marking.cast::<M>()) });
  }
  // SAFETY: `marking` lives until the registration drops it, after the
  // glue's object is detached and deleted; the glue copies `fns`; the
  // isolate's own `CppHeap`, on its thread.
  let glue = unsafe {
    let glue = v8__kun__EmbedderMarking__NEW(marking.cast(), &raw const fns);
    v8__kun__SetEmbedderMarking(heap, glue);
    glue
  };
  EmbedderMarkingRegistration {
    isolate: isolate.as_real_ptr(),
    heap,
    glue,
    marking: marking.cast(),
    drop_marking: drop_box::<M>,
  }
}

impl Drop for EmbedderMarkingRegistration {
  fn drop(&mut self) {
    // SAFETY: on the isolate's thread, before the isolate goes (as
    // `set_embedder_marking`'s caller promised). Once V8's GC that runs is
    // finished (its concurrent markers joined) and the heap no longer has
    // the glue's marking, no V8 thread calls it: it is deleted, and the
    // marking with it (its locals went with V8's marking states).
    unsafe {
      v8__kun__FinishMarking(self.isolate);
      v8__kun__SetEmbedderMarking(self.heap, ptr::null_mut());
      v8__kun__EmbedderMarking__DELETE(self.glue);
      (self.drop_marking)(self.marking);
    }
  }
}

/// # Safety
///
/// `data` is a registration's marking, alive while V8 calls.
unsafe fn marking<'a, M>(data: *mut c_void) -> &'a M {
  // SAFETY: as the caller promises.
  unsafe { &*data.cast::<M>() }
}

unsafe extern "C" fn start<M: EmbedderMarking>(data: *mut c_void) {
  // SAFETY: V8 calls with the registration's data.
  from_cxx(|| unsafe { marking::<M>(data) }.start());
}

unsafe extern "C" fn advance<M: EmbedderMarking>(
  data: *mut c_void,
  max_duration_ms: f64,
  in_atomic_pause: bool,
) -> bool {
  from_cxx(|| {
    // SAFETY: as `start`.
    unsafe { marking::<M>(data) }.advance(max_duration_ms, in_atomic_pause)
  })
}

unsafe extern "C" fn is_done<M: EmbedderMarking>(data: *mut c_void) -> bool {
  // SAFETY: as `start`.
  from_cxx(|| unsafe { marking::<M>(data) }.is_done())
}

unsafe extern "C" fn end<M: EmbedderMarking>(data: *mut c_void) -> usize {
  // SAFETY: as `start`.
  from_cxx(|| unsafe { marking::<M>(data) }.end())
}

unsafe extern "C" fn new_local<M: EmbedderMarking>(
  data: *mut c_void,
) -> *mut c_void {
  from_cxx(|| {
    // SAFETY: as `start`. The local is a box V8 gives back to the locals'
    // functions, then to `delete_local`, once.
    unsafe { marking::<M>(data) }
      .new_local()
      .map_or(ptr::null_mut(), |local| {
        Box::into_raw(Box::new(local)).cast()
      })
  })
}

/// # Safety
///
/// `local` is from `new_local`, not deleted yet, used by one thread at a
/// time (the marking state that holds it).
unsafe fn local<'a, M: EmbedderMarking>(
  local: *mut c_void,
) -> &'a mut M::Local {
  // SAFETY: as the caller promises.
  unsafe { &mut *local.cast::<M::Local>() }
}

// The glue never calls the locals' functions with a null local, and passes
// null data: they don't read it.

unsafe extern "C" fn hand<M: EmbedderMarking>(
  _: *mut c_void,
  handoff: *mut c_void,
  object: *mut c_void,
) {
  from_cxx(|| {
    if let Some(object) = NonNull::new(object) {
      // SAFETY: a local of `new_local`'s, its marking state's.
      M::hand(unsafe { local::<M>(handoff) }, object);
    }
  });
}

unsafe extern "C" fn publish<M: EmbedderMarking>(
  _: *mut c_void,
  handoff: *mut c_void,
) {
  // SAFETY: as `hand`.
  from_cxx(|| M::publish(unsafe { local::<M>(handoff) }));
}

unsafe extern "C" fn is_local_empty<M: EmbedderMarking>(
  _: *mut c_void,
  handoff: *mut c_void,
) -> bool {
  // SAFETY: as `hand`.
  from_cxx(|| M::is_local_empty(unsafe { local::<M>(handoff) }))
}

unsafe extern "C" fn delete_local<M: EmbedderMarking>(
  _: *mut c_void,
  handoff: *mut c_void,
) {
  // SAFETY: a local of `new_local`'s, deleted once.
  from_cxx(|| drop(unsafe { Box::from_raw(handoff.cast::<M::Local>()) }));
}

/// Tells V8 the embedder heap's used bytes are `used` now: it counts them
/// with cppgc's, and on growth may start, advance or finish a major GC now,
/// calling the embedder marking.
pub fn report_embedder_used_size(isolate: &mut Isolate, used: usize) {
  // SAFETY: the isolate, on its thread.
  unsafe { v8__kun__ReportEmbedderUsedSize(isolate.as_real_ptr(), used) };
}

// --- Wrappers ----------------------------------------------------------------

/// Wraps `wrapper` with the embedder heap's `object`, tagged `tag`: V8
/// hands `object` to the embedder marking ([`EmbedderMarking::hand`]) as
/// it marks the wrapper.
///
/// # Safety
///
/// `object` is what the embedder marking takes it for, for as long as the
/// wrapper lives.
///
/// # Panics
///
/// (V8's `CHECK`, which aborts.) Unless `wrapper` is an API wrapper (an
/// object of a `FunctionTemplate`'s) and `tag` is one of
/// [`FIRST_TAG`]`..=`[`LAST_TAG`].
pub unsafe fn wrap(
  isolate: &mut Isolate,
  wrapper: Local<Object>,
  object: NonNull<c_void>,
  tag: u16,
) {
  // SAFETY: the isolate, on its thread, and a handle of it; the patch
  // checks the rest.
  unsafe {
    v8__kun__Wrap(isolate.as_real_ptr(), &*wrapper, object.as_ptr(), tag);
  }
}

/// The object `wrapper` holds with `tag` ([`wrap`]), if it holds one with
/// that tag: none for another tag's, or for an object that isn't an API
/// wrapper (the patch checks).
pub fn unwrap(
  isolate: &mut Isolate,
  wrapper: Local<Object>,
  tag: u16,
) -> Option<NonNull<c_void>> {
  // kun: an API wrapper (every wrapper made from a template) read here, as
  // V8's header inlines `Object::Unwrap`, not a C++ call: a getter's whole
  // floor is a few of those. Debug builds check it against the glue's.
  let object = match unsafe { unwrap_inline(isolate, wrapper, tag) } {
    Some(object) => object,
    // Another object (V8 checks what it is).
    None => NonNull::new(unsafe {
      v8__kun__Unwrap(isolate.as_real_ptr(), &*wrapper, tag)
    }),
  };
  debug_assert_eq!(object, unsafe {
    NonNull::new(v8__kun__Unwrap(isolate.as_real_ptr(), &*wrapper, tag))
  });
  object
}

/// `wrapper`'s object if it is an API wrapper, read as V8's header reads
/// it (`Internals::CanHaveInternalField`, `internal::ReadCppHeapPointerField`):
/// its map's instance type checked, then its `CppHeapPointerHandle`, the
/// isolate's table's entry, the entry's tag. `None` for another object
/// (`Some(None)` for an API wrapper of no object of `tag`'s).
///
/// # Safety
///
/// `isolate` is `wrapper`'s, on its thread.
#[inline(always)]
unsafe fn unwrap_inline(
  isolate: &Isolate,
  wrapper: Local<Object>,
  tag: u16,
) -> Option<Option<NonNull<c_void>>> {
  // SAFETY: a handle's location holds its heap object's tagged address,
  // whose map (compressed, in its cage) holds its instance type; an API
  // wrapper's handle field follows its header, and the isolate's table
  // holds its entry, read atomically as V8 does.
  let entry = unsafe {
    let object = *(&*wrapper as *const Object).cast::<usize>();
    let field = |offset: usize| object - HEAP_OBJECT_TAG + offset;
    let map = (object & !(CAGE_ALIGNMENT - 1))
      + (field(0) as *const u32).read() as usize;
    let instance_type =
      ((map - HEAP_OBJECT_TAG + MAP_INSTANCE_TYPE_OFFSET) as *const u16).read();
    if instance_type != JS_SPECIAL_API_OBJECT_TYPE
      && !(FIRST_JS_API_OBJECT_TYPE..=LAST_JS_API_OBJECT_TYPE)
        .contains(&instance_type)
    {
      return None;
    }
    let handle = (field(JS_OBJECT_HEADER_SIZE) as *const u32).read();
    let table = *(isolate.as_real_ptr() as *const u8)
      .add(CPP_HEAP_POINTER_TABLE_OFFSET)
      .cast::<*const std::sync::atomic::AtomicUsize>();
    (*table.add((handle >> INDEX_SHIFT) as usize))
      .load(std::sync::atomic::Ordering::Relaxed)
  };
  // The tag in the low 16 bits, shifted by one over the marking bit; the
  // null handle's entry, all zeros, fails it.
  let actual = u32::from(entry as u16);
  let first = u32::from(tag) << TAG_SHIFT;
  Some(if actual == first || actual == first + 1 {
    NonNull::new((entry >> PAYLOAD_SHIFT) as *mut c_void)
  } else {
    None
  })
}

/// kun: where the isolate keeps its `CppHeapPointerTable`'s base
/// (`Internals::kIsolateCppHeapPointerTableOffset`, `include/v8-internal.h`):
/// past the embedder data slots (`kNumIsolateDataSlots`, 4), the external
/// pointer table (`kExternalEntityTableSize`: 4 pointers and 4 `uint32_t`s)
/// and the shared one's address. A V8 update must check it against that
/// header.
#[cfg(target_pointer_width = "64")]
const CPP_HEAP_POINTER_TABLE_OFFSET: usize =
  crate::isolate::EMBEDDER_DATA_OFFSET + 4 * 8 + (4 * 8 + 4 * 4) + 8;
/// A JS object's header (`Internals::kJSObjectHeaderSize`: 3 tagged words,
/// compressed): an API wrapper's `CppHeapPointerHandle` follows it.
const JS_OBJECT_HEADER_SIZE: usize = 3 * 4;
/// A tagged heap object pointer's tag (`kHeapObjectTag`).
const HEAP_OBJECT_TAG: usize = 1;
/// A handle's index's shift (`kExternalPointerIndexShift`, desktop
/// platforms').
const INDEX_SHIFT: u32 = 6;
/// An entry's tag's and payload's shifts (`kCppHeapPointerTagShift`,
/// `kCppHeapPointerPayloadShift`).
const TAG_SHIFT: u32 = 1;
const PAYLOAD_SHIFT: u32 = 16;
/// A map's instance type's offset (`Internals::kMapInstanceTypeOffset`),
/// and the API wrappers' types, which hold a `CppHeapPointerHandle` after
/// their header (`Internals::kJSSpecialApiObjectType`,
/// `kFirstJSApiObjectType`, `kLastJSApiObjectType`).
const MAP_INSTANCE_TYPE_OFFSET: usize = 4 + 4;
const JS_SPECIAL_API_OBJECT_TYPE: u16 = 0x410;
const FIRST_JS_API_OBJECT_TYPE: u16 = 0x422;
const LAST_JS_API_OBJECT_TYPE: u16 = 0x80a;
/// The pointer compression cage's alignment (`kPtrComprCageBaseAlignment`):
/// a heap object's address rounded down to it is its cage's base.
const CAGE_ALIGNMENT: usize = 1 << 32;

/// kun: `string`'s characters if it is a sequential one-byte string, or a
/// thin one (an internalized string's forward) to one, read in place as
/// V8's header reads a string's fields (`Internals::kStringResourceOffset`
/// is the first byte past the header): the map's instance type, the length
/// after the hash, the characters after the length. `None` for another
/// representation (cons, sliced, external, two-byte), which V8's
/// `ValueView` reads. Debug builds check it against `ValueView`'s.
///
/// # Safety
///
/// `string` is `isolate`'s, and no GC runs while the slice lives: a GC may
/// move the string (or free a thin string's forward).
#[inline(always)]
pub unsafe fn one_byte_chars<'s>(
  isolate: &Isolate,
  string: &'s crate::String,
) -> Option<&'s [u8]> {
  // SAFETY: as `unwrap_inline`'s: a handle's location holds its string's
  // tagged address, whose map (compressed, in its cage) holds its instance
  // type; a string's length follows its map and hash, a sequential one's
  // characters its length, a thin one's forward (compressed) its length.
  let chars = unsafe {
    let mut object = *(string as *const crate::String).cast::<usize>();
    let cage = object & !(CAGE_ALIGNMENT - 1);
    let field =
      |object: usize, offset: usize| object - HEAP_OBJECT_TAG + offset;
    let mut forwarded = false;
    loop {
      let map = cage + (field(object, 0) as *const u32).read() as usize;
      let instance_type = ((map - HEAP_OBJECT_TAG + MAP_INSTANCE_TYPE_OFFSET)
        as *const u16)
        .read();
      if instance_type >= FIRST_NONSTRING_TYPE {
        break None;
      }
      match instance_type & STRING_REPRESENTATION_AND_ENCODING {
        SEQ_ONE_BYTE_STRING => {
          let length =
            (field(object, STRING_LENGTH_OFFSET) as *const u32).read();
          break Some(std::slice::from_raw_parts(
            field(object, STRING_HEADER_SIZE) as *const u8,
            length as usize,
          ));
        }
        // An internalized string's forward is never another forward.
        tag if tag & STRING_REPRESENTATION == THIN_STRING && !forwarded => {
          forwarded = true;
          object = cage
            + (field(object, STRING_HEADER_SIZE) as *const u32).read() as usize;
        }
        _ => break None,
      }
    }
  };
  #[cfg(debug_assertions)]
  if let Some(chars) = chars {
    // SAFETY: as the caller promises, `string` is `isolate`'s and alive.
    let view = unsafe { crate::ValueView::new_from_ref(isolate, string) };
    match view.data() {
      crate::ValueViewData::OneByte(bytes) => assert_eq!(chars, bytes),
      crate::ValueViewData::TwoByte(_) => {
        panic!("a one-byte string read as two-byte")
      }
    }
  }
  #[cfg(not(debug_assertions))]
  let _ = isolate;
  chars
}

/// kun: a string's fields (`include/v8-internal.h`, `src/objects/string.h`,
/// `src/objects/instance-type.h`): its length after the map and the hash
/// (pointer compression's 4-byte map), its header's size (a sequential
/// string's characters, a thin string's forward, follow it), the first
/// instance type past the strings' (`Internals::kFirstNonstringType`),
/// and the representation and encoding bits of a string's instance type
/// (`kStringRepresentationMask`, `kStringEncodingMask`).
const STRING_LENGTH_OFFSET: usize = 4 + 4;
const STRING_HEADER_SIZE: usize = 4 + 4 + 4;
const FIRST_NONSTRING_TYPE: u16 = 0x80;
const STRING_REPRESENTATION: u16 = 0x07;
const STRING_REPRESENTATION_AND_ENCODING: u16 = 0x0f;
/// `kSeqStringTag | kOneByteStringTag`, `kThinStringTag`.
const SEQ_ONE_BYTE_STRING: u16 = 0x08;
const THIN_STRING: u16 = 0x05;

/// A shallow copy of `object` (V8's `Object::Clone`): a new wrapper from a
/// boilerplate one, as Blink's `V8PerContextData::CreateWrapperFromCache`,
/// instead of instantiating its template again.
pub fn clone_object<'s>(
  scope: &PinScope<'s, '_, ()>,
  object: Local<Object>,
) -> Local<'s, Object> {
  // SAFETY: the isolate, on its thread, and a handle of it; the copy is a
  // handle V8 makes in `scope`'s handle scope.
  unsafe {
    scope
      .cast_local(|sd| v8__kun__Object__Clone(sd.get_isolate_ptr(), &*object))
  }
  .expect("V8 copies an object")
}

impl TracedSlot<Object> {
  /// Points the slot at `value`, droppable (`TracedReference::IsDroppable`,
  /// as Blink's main world wrapper): V8's scavenge may drop `value` if it
  /// is unmodified and nothing else holds it, and asks the isolate's
  /// [`RootsHandler`] to reset the slot. While V8 marks, it marks `value`
  /// at once.
  ///
  /// # Safety
  ///
  /// As [`TracedSlot::set`]: on the heap's thread, `scope`'s isolate's;
  /// the slot in its final place, never moved again.
  pub unsafe fn set_droppable<'s>(
    &self,
    scope: &PinScope<'s, '_, ()>,
    value: Local<'s, Object>,
  ) {
    // SAFETY: as the caller promises; `value` a handle in `scope`.
    unsafe {
      v8__kun__TracedReference__SetDroppable(
        scope.get_isolate_ptr(),
        self.raw(),
        &*value,
      );
    }
  }
}

/// A thread's marking of the references in the embedder heap's objects
/// into V8's worklists (`v8::kun::TracedMarker`, cppgc's
/// `ConcurrentUnifiedHeapMarkingVisitor`): made while V8 runs a major GC's
/// marking, on any thread; published when dropped.
pub struct TracedMarker(*mut c_void);

// SAFETY: the C++ marker is made, used and deleted on one thread at a time,
// and holds V8's worklist of its own.
unsafe impl Send for TracedMarker {}

impl TracedMarker {
  /// # Safety
  ///
  /// V8 runs a major GC's marking of `isolate`, which goes on while the
  /// marker lives: the marker is dropped before the embedder marking says
  /// it is done.
  pub unsafe fn new(isolate: UnsafeRawIsolatePtr) -> Self {
    // SAFETY: as the caller promises.
    Self(unsafe { v8__kun__TracedMarker__NEW(isolate.as_real_ptr()) })
  }

  /// Marks the value `slot` refers to, if any: V8 reads it atomically
  /// (`GetSlotThreadSafe`) as the heap's thread may write it.
  pub fn mark<T>(&mut self, slot: &TracedSlot<T>) {
    // SAFETY: a live marker, in the marking it was made in; a live slot.
    unsafe { v8__kun__TracedMarker__Mark(self.0, slot.raw()) };
  }

  /// Publishes what this marker pushed, for V8's other threads.
  pub fn publish(&mut self) {
    // SAFETY: a live marker.
    unsafe { v8__kun__TracedMarker__Publish(self.0) };
  }
}

impl Drop for TracedMarker {
  fn drop(&mut self) {
    // SAFETY: made by `new`, deleted once (publishing its worklist).
    unsafe { v8__kun__TracedMarker__DELETE(self.0) };
  }
}

/// A droppable reference V8's scavenge asks the embedder to reset
/// (`const TracedReference<Value>&`, in `EmbedderRootsHandler::ResetRoot`).
#[repr(C)]
pub struct RootHandle(Opaque);

impl RootHandle {
  /// Resets `slot` if it refers to what this does: whether it did.
  ///
  /// # Safety
  ///
  /// On the heap's thread, the slot's writer's; or in V8's parallel reset
  /// of this handle (`RootsHandler::try_reset_root`), on any of its
  /// threads: the slot is then the one this refers to, whose node V8 lets
  /// the thread free (each thread frees in the blocks it processes).
  pub unsafe fn reset_if_same<T>(&self, slot: &TracedSlot<T>) -> bool {
    // SAFETY: as the caller promises; `self` is V8's, alive for the call.
    unsafe { v8__kun__TracedReference__ResetIfSame(slot.raw(), self) }
  }
}

/// What resets the references to the wrappers V8's scavenge drops
/// (`v8::EmbedderRootsHandler`, Blink's `V8EmbedderRootsHandler`).
pub trait RootsHandler: Sync + 'static {
  /// V8 dropped an unmodified wrapper holding `object` (one of the tags),
  /// which `handle` refers to: on the isolate's thread, in its scavenge.
  fn reset_root(&self, object: NonNull<c_void>, handle: &RootHandle);

  /// As `reset_root`, but on any of V8's threads, in parallel, a handle
  /// each (`TryResetRoot`): whether the reference is reset; if not, V8
  /// calls `reset_root` for it on the isolate's thread.
  fn try_reset_root(
    &self,
    object: NonNull<c_void>,
    handle: &RootHandle,
  ) -> bool {
    let _ = (object, handle);
    false
  }
}

/// A roots handler set on an isolate ([`set_roots_handler`]): unset and
/// dropped when dropped.
pub struct RootsHandlerRegistration {
  isolate: *mut RealIsolate,
  glue: *mut c_void,
  handler: *mut c_void,
  drop_handler: unsafe fn(*mut c_void),
}

/// Sets `handler` as the isolate's roots handler, replacing V8's default
/// (which drops nothing of these).
///
/// # Safety
///
/// The registration is dropped on the isolate's thread, before the isolate
/// is.
pub unsafe fn set_roots_handler<H: RootsHandler>(
  isolate: &mut Isolate,
  handler: Box<H>,
) -> RootsHandlerRegistration {
  unsafe extern "C" fn reset_root<H: RootsHandler>(
    data: *mut c_void,
    object: *mut c_void,
    handle: *const RootHandle,
  ) {
    from_cxx(|| {
      if let Some(object) = NonNull::new(object) {
        // SAFETY: the registration's handler, alive while V8 calls; V8's
        // handle, alive for the call.
        unsafe { (*data.cast::<H>()).reset_root(object, &*handle) };
      }
    });
  }
  unsafe extern "C" fn try_reset_root<H: RootsHandler>(
    data: *mut c_void,
    object: *mut c_void,
    handle: *const RootHandle,
  ) -> bool {
    from_cxx(|| {
      // SAFETY: as `reset_root`'s; `H: Sync`, called from V8's threads.
      NonNull::new(object).is_some_and(|object| unsafe {
        (*data.cast::<H>()).try_reset_root(object, &*handle)
      })
    })
  }
  /// Drops the `Box<H>` at `handler`.
  unsafe fn drop_box<H>(handler: *mut c_void) {
    // SAFETY: as the caller promises: `Box::into_raw` of a `Box<H>`,
    // dropped once.
    drop(unsafe { Box::from_raw(handler.cast::<H>()) });
  }
  let handler = Box::into_raw(handler).cast::<c_void>();
  // SAFETY: the isolate, on its thread; `handler` lives until the glue's
  // object is unset and deleted.
  let glue = unsafe {
    v8__kun__RootsHandler__NEW(
      isolate.as_real_ptr(),
      handler,
      reset_root::<H>,
      try_reset_root::<H>,
    )
  };
  RootsHandlerRegistration {
    isolate: isolate.as_real_ptr(),
    glue,
    handler,
    drop_handler: drop_box::<H>,
  }
}

impl Drop for RootsHandlerRegistration {
  fn drop(&mut self) {
    // SAFETY: on the isolate's thread, before it goes (as
    // `set_roots_handler`'s caller promised): V8 calls the handler only
    // there, in its scavenges, so none runs; unset, then deleted.
    unsafe {
      v8__kun__RootsHandler__DELETE(self.isolate, self.glue);
      (self.drop_handler)(self.handler);
    }
  }
}

// --- Workers -----------------------------------------------------------------

/// A parallel job on V8's platform (`v8::JobTask`).
pub trait JobTask: Send + Sync + 'static {
  /// A worker's run (`JobTask::Run`): works until nothing is left to take,
  /// or `delegate.should_yield()`.
  fn run(&self, delegate: &JobDelegate);

  /// How many workers could work now, `worker_count` of them running it
  /// already (`GetMaxConcurrency`, under the job's lock).
  fn max_concurrency(&self, worker_count: usize) -> usize;
}

/// A run's `v8::JobDelegate`, valid during the run.
#[repr(C)]
pub struct JobDelegate(Opaque);

impl JobDelegate {
  /// The run should return soon: the job was cancelled, or the platform
  /// runs something more urgent.
  pub fn should_yield(&self) -> bool {
    // SAFETY: V8's delegate, during the run.
    unsafe { v8__kun__JobDelegate__ShouldYield(ptr::from_ref(self).cast_mut()) }
  }

  /// `max_concurrency` grew: more workers may help.
  pub fn notify_concurrency_increase(&self) {
    // SAFETY: as above.
    unsafe {
      v8__kun__JobDelegate__NotifyConcurrencyIncrease(
        ptr::from_ref(self).cast_mut(),
      );
    }
  }
}

/// A posted job's `v8::JobHandle`: cancelled when dropped, which returns
/// once every run has (V8's `Cancel`).
pub struct JobHandle(NonNull<c_void>);

// SAFETY: V8's `JobHandle` may be used from any thread (its state is
// behind a lock); its owner deletes it once.
unsafe impl Send for JobHandle {}
// SAFETY: as above; `notify_concurrency_increase` is the only method of
// `&self`.
unsafe impl Sync for JobHandle {}

impl JobHandle {
  /// `max_concurrency` grew: more workers may help.
  pub fn notify_concurrency_increase(&self) {
    // SAFETY: a handle not cancelled yet: only its owner cancels it.
    unsafe { v8__kun__JobHandle__NotifyConcurrencyIncrease(self.0.as_ptr()) };
  }

  /// Stops the job: its runs see `should_yield`, no new one starts, and
  /// this returns once every run has.
  pub fn cancel(self) {
    drop(self);
  }

  /// Stops the job without waiting for its runs, which go on until they
  /// see `should_yield`.
  pub fn cancel_and_detach(self) {
    let handle = self.0;
    std::mem::forget(self);
    // SAFETY: the handle, deleted once (forgotten above).
    unsafe { v8__kun__JobHandle__CancelAndDetach(handle.as_ptr()) };
  }
}

impl Drop for JobHandle {
  fn drop(&mut self) {
    // SAFETY: the handle, deleted once.
    unsafe { v8__kun__JobHandle__Cancel(self.0.as_ptr()) };
  }
}

/// Posts `task` to V8's `platform`, to run on its workers while
/// `max_concurrency` asks for them (V8's `PostJob`), `user_blocking` if a
/// thread waits for it. V8 drops the task once it is done with it, which
/// may be after the handle is cancelled, on any thread.
///
/// # Safety
///
/// `platform` outlives the job (V8's platform lives as long as the
/// process, once initialized).
pub unsafe fn post_job(
  platform: &Platform,
  user_blocking: bool,
  task: Box<dyn JobTask>,
) -> JobHandle {
  /// # Safety
  ///
  /// `data` is `post_job`'s boxed task, not dropped yet.
  unsafe fn task_of<'a>(data: *mut c_void) -> &'a dyn JobTask {
    // SAFETY: as the caller promises.
    unsafe { &**data.cast::<Box<dyn JobTask>>() }
  }
  unsafe extern "C" fn run(data: *mut c_void, delegate: *mut JobDelegate) {
    // SAFETY: V8 calls with the task's data and the run's delegate.
    from_cxx(|| unsafe { task_of(data).run(&*delegate) });
  }
  unsafe extern "C" fn max_concurrency(
    data: *mut c_void,
    worker_count: usize,
  ) -> usize {
    // SAFETY: as `run`.
    from_cxx(|| unsafe { task_of(data) }.max_concurrency(worker_count))
  }
  unsafe extern "C" fn drop_task(data: *mut c_void) {
    // SAFETY: the task's box, dropped once, as V8 deletes the task.
    from_cxx(|| {
      drop(unsafe { Box::from_raw(data.cast::<Box<dyn JobTask>>()) })
    });
  }
  let fns = JobFns {
    run,
    max_concurrency,
    drop: drop_task,
  };
  let data = Box::into_raw(Box::new(task)).cast::<c_void>();
  // SAFETY: the platform outlives the job, as the caller promises; `data`
  // is owned by the task from here on; the glue copies `fns`.
  let handle = unsafe {
    v8__kun__Platform__PostJob(platform, user_blocking, data, &raw const fns)
  };
  JobHandle(NonNull::new(handle).expect("V8 posts the job"))
}
