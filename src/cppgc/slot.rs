// kun: GC pointer slots that are sound to trace from a concurrent marker
// thread while the heap's thread writes them: `MemberSlot`,
// `WeakMemberSlot` and `TracedSlot`. They sit next to upstream's `Member`,
// `WeakMember` and `TracedReference`, which they don't change, and call the
// same C functions.
//
// Why not upstream's types:
// - **Aliasing.** Their writes take `&mut self` (`set`, `reset`) and their
//   trace `&self`. Under concurrent marking a marker thread traces a slot
//   while the heap's thread writes it, so that would put a `&` and a `&mut`
//   to the same bytes on two threads at once: undefined behavior in Rust,
//   even though both accesses are atomic in C++ (`SetRawAtomic` in
//   `operator=`, `GetAtomic` in `Visitor::Trace`, `SetSlotThreadSafe` /
//   `GetSlotThreadSafe` for `TracedReference`). These slots keep the bytes in
//   an `UnsafeCell` and hand C++ a raw pointer for every write and trace, as
//   `AtomicPtr` does. The one Rust reference ever made into them is the
//   `&AtomicPtr` of `TracedSlot::is_empty`, which, like any reference to an
//   atomic, allows the concurrent atomic stores C++ makes.
// - **Alignment.** Upstream's storage is a `[u8; N]`, so it is 1-aligned and
//   may land at any offset (after a `bool`, in an enum variant). C++
//   accesses the slot as a `std::atomic`, which must be aligned. These slots
//   are aligned to their size.
//
// Every method that takes `&self` is `unsafe`, with the thread it may run on
// in its contract; that is what makes `unsafe impl Sync` sound.
//
// **Publishing new objects.** No fence is needed between writing a new
// object and storing a pointer to it in a slot: `make_garbage_collected`
// writes the Rust data in `RustObj`'s constructor (see `RustObjInit` in
// `support.h`), before cppgc's "fully constructed" release store, and a
// marker traces an object only after reading that bit with acquire.
//
// `WeakPersistentSlot`, at the end, is here for another reason: not
// concurrent marking (persistents are roots, which only the heap's thread
// reads, in the GC's pause) but where it lives. Upstream's `WeakPersistent`
// is one C++ `new` each; this one is built in memory the caller owns, and
// pinned there once set.

use std::cell::UnsafeCell;
use std::ffi::c_void;
use std::marker::{PhantomData, PhantomPinned};
use std::mem::{MaybeUninit, align_of, size_of};
use std::pin::Pin;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicPtr, AtomicU32, AtomicUsize, Ordering};

use super::{
  GarbageCollected, GetRustObj, MemberInner, RustObj, UnsafePtr, Visitor,
  WeakMemberInner, WeakPersistentInner, cppgc__Member__Assign,
  cppgc__Member__CONSTRUCT, cppgc__Member__DESTRUCT, cppgc__Member__Get,
  cppgc__Visitor__Trace__Member, cppgc__Visitor__Trace__TracedReference,
  cppgc__Visitor__Trace__WeakMember, cppgc__WeakMember__Assign,
  cppgc__WeakMember__CONSTRUCT, cppgc__WeakMember__DESTRUCT,
  cppgc__WeakMember__Get, cppgc__WeakPersistent__Assign,
  cppgc__WeakPersistent__CONSTRUCT_IN_PLACE,
  cppgc__WeakPersistent__DESTRUCT_IN_PLACE, cppgc__WeakPersistent__Get,
  get_value_from_rust_obj,
};
use crate::isolate::RealIsolate;
use crate::scope::GetIsolate;
use crate::{Data, Local, PinScope, TracedReference};

// Upstream declares these in `handle.rs`, private to it; the same
// signatures here. `cppgc__CageBase` is the fork's own.
unsafe extern "C" {
  fn cppgc__CageBase() -> usize;
  fn v8__TracedReference__CONSTRUCT(this: *mut TracedReference<Data>);
  fn v8__TracedReference__DESTRUCT(this: *mut TracedReference<Data>);
  fn v8__TracedReference__Reset(
    this: *mut TracedReference<Data>,
    isolate: *mut RealIsolate,
    data: *mut Data,
  );
  fn v8__TracedReference__Get(
    this: *const TracedReference<Data>,
    isolate: *mut RealIsolate,
  ) -> *const Data;
}

// --- MemberSlot / WeakMemberSlot ---------------------------------------------

/// `sizeof(cppgc::Member<RustObj>)`: 4 with cppgc's pointer compression
/// (which V8 turns on with its caged heap, the default on x64 and arm64), 8
/// without. The C++ object is one (possibly compressed) pointer, so its
/// alignment equals its size.
const MEMBER_SIZE: usize = crate::binding::cppgc__Member_SIZE;
const _: () = assert!(crate::binding::cppgc__WeakMember_SIZE == MEMBER_SIZE);

trait AlignedAs {
  type Unit: Copy;
}
struct Bytes<const N: usize>;
impl AlignedAs for Bytes<4> {
  type Unit = u32;
}
impl AlignedAs for Bytes<8> {
  type Unit = u64;
}

/// How far a compressed `Member` is shifted (`kPointerCompressionShift`).
const COMPRESSION_SHIFT: usize =
  crate::binding::cppgc__POINTER_COMPRESSION_SHIFT;
const _: () = assert!((MEMBER_SIZE == 4) == (COMPRESSION_SHIFT != 0));

/// The cage base, once read: it is set when the first heap reserves its
/// cage, before any `Member` exists, and never changes.
static CAGE_BASE: AtomicUsize = AtomicUsize::new(0);

#[inline]
fn cage_base() -> usize {
  match CAGE_BASE.load(Ordering::Relaxed) {
    0 => {
      // SAFETY: only reads a global.
      let base = unsafe { cppgc__CageBase() };
      CAGE_BASE.store(base, Ordering::Relaxed);
      base
    }
    base => base,
  }
}

/// What a `Member`'s storage points to: `Member::Get`, done here instead of
/// with a call into C++ for each read (C++ inlines it): a compressed pointer
/// sign-extended, shifted and masked with the cage base
/// (`CompressedPointer::Decompress`), which keeps null and the sentinel as
/// they are; else the pointer itself. A relaxed atomic load, as plain as
/// C++'s, and race-free with a marker thread's atomic one.
///
/// # Safety
///
/// `storage` is a live `Member`'s or `WeakMember`'s.
#[inline]
unsafe fn load_member(storage: *const MemberStorage) -> *mut RustObj {
  if MEMBER_SIZE == 4 {
    // SAFETY: per the caller; aligned like the `u32` it holds.
    let compressed =
      unsafe { (*storage.cast::<AtomicU32>()).load(Ordering::Relaxed) };
    let mask =
      ((compressed as i32 as i64 as u64) << COMPRESSION_SHIFT) as usize;
    (mask & cage_base()) as *mut RustObj
  } else {
    // SAFETY: per the caller; aligned like the pointer it holds.
    unsafe { (*storage.cast::<AtomicPtr<RustObj>>()).load(Ordering::Relaxed) }
  }
}

/// A `Member`'s bytes, aligned like it. Any other size fails to compile (no
/// `AlignedAs` impl).
#[repr(C)]
struct MemberStorage {
  _align: [<Bytes<MEMBER_SIZE> as AlignedAs>::Unit; 0],
  _bytes: [u8; MEMBER_SIZE],
}
const _: () = assert!(
  size_of::<MemberStorage>() == MEMBER_SIZE
    && align_of::<MemberStorage>() == MEMBER_SIZE
);

macro_rules! member_slot {
  (
    $(#[$doc:meta])*
    $name:ident, $inner:ident,
    construct: $construct:ident, destruct: $destruct:ident,
    get: $get:ident, assign: $assign:ident, trace: $trace:ident $(,)?
  ) => {
    $(#[$doc])*
    ///
    /// `Sync`, so a GC object holding one can be traced by a marker thread
    /// while the heap's thread writes it. Every method that takes `&self`
    /// passes C++ a raw pointer into an `UnsafeCell` and is `unsafe`, with the
    /// thread it may run on in its contract.
    pub struct $name<T: GarbageCollected> {
      storage: UnsafeCell<MemberStorage>,
      _phantom: PhantomData<T>,
    }

    // SAFETY: the only data is the C++ object in the `UnsafeCell`, and no
    // Rust reference to it is ever created: every access is a C++ call
    // through a raw pointer, or (`get`, `load_member`) a relaxed atomic load
    // through one. Of those, only `trace` may run off the heap's thread (on
    // a marker thread), and it only reads, with an atomic load
    // (`GetAtomic`). Writes (`assign`, `clear`, `copy_from`) are atomic
    // stores (`operator=`'s `SetRawAtomic`) on the heap's thread, which is
    // also the only thread that does the plain reads (`get`, `copy_from`).
    // So no two threads race. The GC's own writes (clearing a dead weak
    // slot) happen in the final pause, when neither the mutator nor marker
    // threads touch slots. Safe code can't call any of these, so sharing
    // `&Self` gives it nothing.
    unsafe impl<T: GarbageCollected> Sync for $name<T> {}

    impl<T: GarbageCollected> $name<T> {
      /// An empty (null) slot.
      ///
      /// cppgc must have set up its cage (the first heap does) before
      /// this is called: the slot holds a pointer compressed against the
      /// cage base, and V8's debug checks reject a slot made before.
      pub fn empty() -> Self {
        let mut storage = MaybeUninit::<MemberStorage>::uninit();
        // SAFETY: constructs the C++ object in place. It holds no address
        // of its own (a compressed pointer is relative to the cage base),
        // so moving it afterwards is fine; upstream's `Member` relies on
        // the same.
        let storage = unsafe {
          $construct(storage.as_mut_ptr().cast(), std::ptr::null_mut());
          storage.assume_init()
        };
        Self {
          storage: UnsafeCell::new(storage),
          _phantom: PhantomData,
        }
      }

      fn raw(&self) -> *mut $inner {
        self.storage.get().cast()
      }

      /// `*this = target`: an atomic store plus the write barrier.
      ///
      /// # Safety
      ///
      /// On the heap's thread, with `target` on the same heap.
      pub unsafe fn assign(&self, target: &impl GetRustObj<T>) {
        // SAFETY: a live slot, per the caller. No fence: see "Publishing
        // new objects" at the top.
        unsafe { $assign(self.raw(), target.get_rust_obj()) }
      }

      /// `*this = nullptr`.
      ///
      /// # Safety
      ///
      /// On the heap's thread.
      pub unsafe fn clear(&self) {
        // SAFETY: a live slot; the barrier does nothing for null.
        unsafe { $assign(self.raw(), std::ptr::null_mut()) }
      }

      /// The target, or `None` if empty (or, for a weak slot, collected).
      ///
      /// # Safety
      ///
      /// On the heap's thread. The target must stay alive while the
      /// reference is used: this slot is traced by its owner and no GC
      /// runs in between (or the target is otherwise kept alive).
      pub unsafe fn get(&self) -> Option<&T> {
        // SAFETY: per the caller.
        let target = unsafe { self.get_ptr()? };
        // SAFETY: the reference outlives the local `UnsafePtr`, but not the
        // object, per the caller.
        Some(unsafe { &*(target.as_ref() as *const T) })
      }

      /// The target as a pointer, e.g. to root it with a `Persistent`.
      ///
      /// # Safety
      ///
      /// As for `get`, and the pointer must stay on the stack (or be moved
      /// into a root) like any `UnsafePtr`.
      pub unsafe fn get_ptr(&self) -> Option<UnsafePtr<T>> {
        // SAFETY: a live slot, read on the heap's thread, the only one that
        // writes slots (the GC's own writes, clearing weak slots, happen in
        // a pause); marker threads only read.
        let target = unsafe { load_member(self.storage.get()) };
        // kun: a debug build checks the read against C++'s own.
        if cfg!(debug_assertions) {
          assert_eq!(target, unsafe { $get(self.raw()) }, "Member::Get differs");
        }
        // A slot only ever points to objects of type `T` (it is typed), and
        // the target is alive per the caller.
        Some(UnsafePtr {
          pointer: NonNull::new(target)?,
          _phantom: PhantomData,
        })
      }

      /// `visitor->Trace(*this)`. Safe to run on a marker thread while the
      /// heap's thread writes: both sides are atomic in C++.
      ///
      /// # Safety
      ///
      /// Only from the `trace` of the GC object this slot is a field of.
      /// (cppgc keeps the address of a weak slot to clear it later, so the
      /// slot must live as long as that object.)
      pub unsafe fn trace(&self, visitor: &mut Visitor) {
        // SAFETY: a live slot, per the caller.
        unsafe { $trace(visitor, self.raw()) }
      }
    }

    impl<T: GarbageCollected> Drop for $name<T> {
      fn drop(&mut self) {
        // SAFETY: a live slot, destroyed once.
        unsafe { $destruct(self.raw()) }
      }
    }
  };
}

member_slot! {
  /// A strong pointer from a GC object to another (`cppgc::Member`).
  MemberSlot, MemberInner,
  construct: cppgc__Member__CONSTRUCT, destruct: cppgc__Member__DESTRUCT,
  get: cppgc__Member__Get, assign: cppgc__Member__Assign,
  trace: cppgc__Visitor__Trace__Member,
}

member_slot! {
  /// A weak pointer from a GC object to another (`cppgc::WeakMember`):
  /// cleared by the GC when its target dies.
  WeakMemberSlot, WeakMemberInner,
  construct: cppgc__WeakMember__CONSTRUCT, destruct: cppgc__WeakMember__DESTRUCT,
  get: cppgc__WeakMember__Get, assign: cppgc__WeakMember__Assign,
  trace: cppgc__Visitor__Trace__WeakMember,
}

impl<T: GarbageCollected> MemberSlot<T> {
  /// `*this = *other`. `self` and `other` may be the same slot.
  ///
  /// # Safety
  ///
  /// On the heap's thread.
  pub unsafe fn copy_from(&self, other: &MemberSlot<T>) {
    // SAFETY: live slots. The read is not atomic, but only this thread
    // writes slots, so it can't race.
    unsafe {
      cppgc__Member__Assign(self.raw(), cppgc__Member__Get(other.raw()))
    }
  }
}

// --- TracedSlot --------------------------------------------------------------

/// `sizeof(v8::TracedReference<v8::Data>)`: one pointer (`location_`).
const TRACED_SIZE: usize = crate::binding::v8__TracedReference_SIZE;
const _: () = assert!(TRACED_SIZE == size_of::<usize>());

/// A `TracedReference`'s bytes, aligned like it (a pointer).
#[repr(C)]
struct TracedStorage {
  _align: [usize; 0],
  _bytes: [u8; TRACED_SIZE],
}
const _: () = assert!(
  size_of::<TracedStorage>() == TRACED_SIZE
    && align_of::<TracedStorage>() == align_of::<usize>()
);

/// A GC object's pointer to a JS value of type `T` (`v8::TracedReference`),
/// traced with the object in the unified heap: the value lives as long as
/// the object does and the slot points to it.
///
/// **Concurrent marking.** V8 makes `TracedReference` safe to trace from a
/// marker thread while the heap's thread writes it, and this relies on
/// exactly that, as Blink does (`v8/src/handles/traced-handles*.{h,cc}`):
/// - The slot holds one pointer to a node in V8's traced-handle table.
///   `Reset` stores it with `SetSlotThreadSafe` (relaxed) and the marker
///   loads it with `GetSlotThreadSafe` (relaxed).
/// - The node's contents are published separately: `TracedNode::Publish`
///   stores the object with release, and `TracedHandles::Mark` loads it with
///   acquire.
/// - Assigning while marking is running marks the target right away (black
///   allocation and `WriteBarrier::MarkingFromTracedHandle`), so a marker
///   that read the slot before the store misses nothing.
/// - Resetting while marking only clears the node's object (atomically); the
///   node itself is freed in the final pause, so a marker holding the old
///   slot pointer reads a live node.
///
/// So unlike `MemberSlot`, no fences are needed here: the ordering is V8's.
///
/// **Not movable once set.** V8 may remember the slot's address in its node
/// (old-to-new tracking, for cppgc's young generation), so a non-empty
/// `TracedReference` must stay where it is. Slots are only created empty,
/// and `set` requires the slot to be in its final place, a field of a GC
/// object on the heap (cppgc doesn't move objects).
///
/// **Only on a heap attached to an isolate.** cppgc traces a
/// `TracedReference` by casting the visitor to `v8::JSVisitor`
/// (`TraceTrait<v8::TracedReference<T>>` in `v8/include/v8-cppgc.h`), which
/// only the unified heap's visitors are. A detached heap's visitor is a plain
/// `cppgc::Visitor`, so `trace` skips empty slots without calling into C++,
/// and setting a slot needs an isolate: on a detached heap every slot stays
/// empty.
///
/// `Sync`, so a GC object holding one can be traced by a marker thread while
/// the heap's thread writes it. Every method that takes `&self` passes C++ a
/// raw pointer into an `UnsafeCell` and is `unsafe`, with the thread it may
/// run on in its contract.
pub struct TracedSlot<T> {
  storage: UnsafeCell<TracedStorage>,
  _phantom: PhantomData<T>,
}

// SAFETY: the only data is the C++ object in the `UnsafeCell`. Writes
// (`set`) are on the heap's thread and store the pointer with
// `SetSlotThreadSafe` (an atomic store); `get` reads on the same thread.
// Only `trace` runs on marker threads, and it reads the pointer atomically
// (`is_empty`, and `GetSlotThreadSafe` in C++); see the type docs for why
// the node it points to is safe to read there. The GC's own writes happen
// in the final pause. Safe code can't call any of these, so sharing `&Self`
// gives it nothing.
unsafe impl<T> Sync for TracedSlot<T> {}

impl<T> TracedSlot<T> {
  /// An empty slot.
  pub fn empty() -> Self {
    let mut storage = MaybeUninit::<TracedStorage>::uninit();
    // SAFETY: constructs the C++ object in place: a null pointer, which V8
    // knows nothing about, so moving it afterwards is fine.
    let storage = unsafe {
      v8__TracedReference__CONSTRUCT(storage.as_mut_ptr().cast());
      storage.assume_init()
    };
    Self {
      storage: UnsafeCell::new(storage),
      _phantom: PhantomData,
    }
  }

  fn raw(&self) -> *mut TracedReference<Data> {
    self.storage.get().cast()
  }

  /// Whether the slot is empty, read atomically: the C++ object is a single
  /// pointer (`location_`), null when empty, as `IsEmptyThreadSafe` reads it.
  fn is_empty(&self) -> bool {
    // SAFETY: the storage is the C++ object's one pointer, aligned
    // (`TracedStorage`), and every write to it is atomic.
    let location = unsafe { &*self.storage.get().cast::<AtomicPtr<c_void>>() };
    location.load(Ordering::Relaxed).is_null()
  }

  /// Points the slot at `value`, or empties it (`TracedReference::Reset`).
  /// While marking is running, V8 marks `value` right away.
  ///
  /// # Safety
  ///
  /// On the heap's thread, which `scope`'s isolate must be the heap of. The
  /// slot must be in its final place: a field of a GC object already on the
  /// heap, never moved again.
  pub unsafe fn set<'s>(
    &self,
    scope: &PinScope<'s, '_, ()>,
    value: Option<Local<'s, T>>,
  ) {
    let value = value
      .map_or(std::ptr::null_mut(), |v| v.as_non_null().as_ptr())
      .cast();
    // SAFETY: a live slot in its final place, on the heap's thread, per the
    // caller; `value` is a handle in `scope`.
    unsafe {
      v8__TracedReference__Reset(self.raw(), scope.get_isolate_ptr(), value)
    }
  }

  /// The value, or `None` if the slot is empty.
  ///
  /// # Safety
  ///
  /// On the heap's thread, which `scope`'s isolate must be the heap of.
  pub unsafe fn get<'s>(
    &self,
    scope: &PinScope<'s, '_, ()>,
  ) -> Option<Local<'s, T>> {
    // SAFETY: on the only thread that writes the slot, per the caller; the
    // `Local` is created in `scope`.
    unsafe {
      scope.cast_local(|sd| {
        v8__TracedReference__Get(self.raw(), sd.get_isolate_ptr()) as *const T
      })
    }
  }

  /// `visitor->Trace(*this)` if the slot isn't empty. Safe to run on a
  /// marker thread while the heap's thread writes: see the type docs.
  ///
  /// # Safety
  ///
  /// Only from the `trace` of the GC object this slot is a field of, on a
  /// heap attached to an isolate if the slot isn't empty.
  pub unsafe fn trace(&self, visitor: &mut Visitor) {
    if self.is_empty() {
      return;
    }
    // SAFETY: a live, non-empty slot, so (per the caller) on the unified
    // heap, whose visitor is a `v8::JSVisitor` as cppgc's cast assumes.
    unsafe { cppgc__Visitor__Trace__TracedReference(visitor, self.raw()) }
  }
}

impl<T> Drop for TracedSlot<T> {
  fn drop(&mut self) {
    // SAFETY: a live slot, destroyed once. Disposing a non-empty reference
    // from a finalizer is allowed: during sweeping V8 leaves the node for the
    // next cycle (`TracedHandles::Destroy`).
    unsafe { v8__TracedReference__DESTRUCT(self.raw()) }
  }
}

// --- WeakPersistentSlot ------------------------------------------------------

/// `sizeof(cppgc::WeakPersistent<RustObj>)`: `raw_` and `node_`, plus a
/// `SourceLocation` when cppgc keeps object names (`KeepLocationPolicy`; on
/// by default on desktop builds, release ones too: 32 bytes on x64). Read
/// from the build for that reason.
const WEAK_PERSISTENT_SIZE: usize = crate::binding::cppgc__WeakPersistent_SIZE;
const _: () = assert!(
  crate::binding::cppgc__WeakPersistent_ALIGN == align_of::<usize>()
);

/// A `WeakPersistent`'s bytes, aligned like it (pointers).
#[repr(C)]
struct WeakPersistentStorage {
  _align: [usize; 0],
  _bytes: [u8; WEAK_PERSISTENT_SIZE],
}

/// A weak pointer from memory off the GC heap to a GC object
/// (`cppgc::WeakPersistent`), built where the caller puts it instead of on
/// the C++ heap: a root the GC clears when its target dies.
///
/// **Not movable once set.** Setting an empty slot takes a node from the
/// heap's persistent region, and the node records the slot's address: the
/// GC reads the slot there and clears it there. So `set` and `clear` take
/// `Pin<&Self>`: a pinned slot stays where it is until it is dropped, which
/// gives the node back first. An empty slot has no node (cppgc takes one
/// only for a valid pointer and gives it back when cleared), so `empty`
/// makes one by value, to be moved into place and pinned there.
///
/// Neither `Send` nor `Sync`: only on the thread of the heap its targets
/// are on, as any persistent. No marker thread reads it (persistents are
/// roots, which the heap's thread reads in the GC's pause), so its reads and
/// writes need no atomics; the bytes are in an `UnsafeCell` because the GC
/// clears them while the slot is shared.
pub struct WeakPersistentSlot<T: GarbageCollected> {
  storage: UnsafeCell<MaybeUninit<WeakPersistentStorage>>,
  _phantom: PhantomData<*const T>,
  _pinned: PhantomPinned,
}

impl<T: GarbageCollected> WeakPersistentSlot<T> {
  /// An empty slot.
  pub fn empty() -> Self {
    let mut storage = MaybeUninit::<WeakPersistentStorage>::uninit();
    // SAFETY: constructs the C++ object in place, empty: it has no node, so
    // nothing records its address and moving it afterwards is fine.
    unsafe { cppgc__WeakPersistent__CONSTRUCT_IN_PLACE(storage.as_mut_ptr().cast()) };
    Self {
      storage: UnsafeCell::new(storage),
      _phantom: PhantomData,
      _pinned: PhantomPinned,
    }
  }

  fn raw(&self) -> *mut WeakPersistentInner {
    self.storage.get().cast()
  }

  /// Points the slot at `target`, or empties it if `target` is null
  /// (`operator=`). A slot that was empty takes a node, which records this
  /// address, hence the pin.
  pub fn set(self: Pin<&Self>, target: &impl GetRustObj<T>) {
    // SAFETY: a live slot, pinned: it stays at this address until dropped.
    unsafe { cppgc__WeakPersistent__Assign(self.raw(), target.get_rust_obj()) }
  }

  /// Empties the slot, giving its node back.
  pub fn clear(self: Pin<&Self>) {
    // SAFETY: a live slot, pinned.
    unsafe { cppgc__WeakPersistent__Assign(self.raw(), std::ptr::null_mut()) }
  }

  /// The target, or `None` if the slot is empty or the target was
  /// collected.
  ///
  /// # Safety
  ///
  /// The target must stay alive while the reference is used: no GC runs in
  /// between, or it is otherwise kept alive.
  pub unsafe fn get(&self) -> Option<&T> {
    let target = self.get_rust_obj();
    if target.is_null() {
      return None;
    }
    // SAFETY: the slot only ever points to `T`s, alive per the caller.
    Some(unsafe { &*get_value_from_rust_obj(target) })
  }

  /// The slot at `index` of pinned `slots`, pinned too: an element of a
  /// pinned slice never moves while the slice is pinned.
  pub fn index(slots: Pin<&[Self]>, index: usize) -> Pin<&Self> {
    // SAFETY: structural pinning of the elements; nothing gives out a
    // `&mut` to one.
    unsafe { slots.map_unchecked(|slots| &slots[index]) }
  }
}

impl<T: GarbageCollected> GetRustObj<T> for WeakPersistentSlot<T> {
  fn get_rust_obj(&self) -> *mut RustObj {
    // `PersistentBase`'s first field, the raw pointer (null once the GC
    // cleared it, in a pause on this thread), read here as `Persistent`'s
    // is. A debug build checks it against C++'s own.
    // SAFETY: a live slot; the read can't race (this thread only).
    let target = unsafe { self.storage.get().cast::<*mut RustObj>().read() };
    if cfg!(debug_assertions) {
      // SAFETY: a live slot.
      let cpp = unsafe { cppgc__WeakPersistent__Get(self.raw()) };
      assert_eq!(target, cpp, "WeakPersistent::Get differs");
    }
    target
  }
}

impl<T: GarbageCollected> Drop for WeakPersistentSlot<T> {
  fn drop(&mut self) {
    // SAFETY: a live slot, destroyed once, where it was set: a pinned value
    // is dropped in place, and an unpinned one was never set (so has no
    // node). Gives the node back if the slot holds one.
    unsafe { cppgc__WeakPersistent__DESTRUCT_IN_PLACE(self.raw()) }
  }
}

impl<T: GarbageCollected> std::fmt::Debug for WeakPersistentSlot<T> {
  fn fmt(&self, fmt: &mut std::fmt::Formatter) -> std::fmt::Result {
    fmt.debug_struct("WeakPersistentSlot").finish()
  }
}
