// kun: wasm GC objects on cppgc and a weak table of a Store's objects, for
// kun's wasm engine (kun's RFC 0001, Q15 and Q16). The objects and the
// table are C++ classes in `src/binding.cc`; this is their FFI.
//
// - `WasmGcObject`: a wasm struct or array, a header (type index, length)
//   and its fields, which C++ traces by a type table (`set_types`). A
//   reference field is a 32-bit `Member` to a wasm object (`RefKind::Ref`),
//   to a Rust GC object such as a DOM node (`RefKind::HostRef`), or an
//   i31-or-wasm-object (`RefKind::AnyRef`, the i31 being `value << 1 | 1`).
//   Objects are 16-aligned so that a compressed pointer to one
//   (`address >> 3`) has its low bit clear.
// - `WasmMemberSlot`, `WeakObjectTableSlot`: strong `Member`s to those, as
//   fields of Rust GC objects (a DOM node pointing to a wasm object, a
//   Store's holder to its table).
// - `WeakObjectTable`: the objects a Store allocated or was handed, weak
//   while the Store isn't running and strong while it is (`enter`/`exit`).
//
// Fields are reached by raw pointers (`WasmGcObject::fields`); reference
// fields are written only through `set_*` (atomic, with the barrier) and
// i31s with an atomic store, since marker threads read them.

use super::{AllocationHandle, GarbageCollected, GetRustObj, Visitor};
use crate::binding::RustObj;
use crate::support::Opaque;
use std::cell::UnsafeCell;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU32, Ordering};

unsafe extern "C" {
  fn cppgc__WasmGc__SetTypes(types: *const WasmGcType);
  fn cppgc__WasmGcObject__FIELDS_OFFSET() -> usize;
  fn cppgc__WasmGcObject__New(
    handle: *mut AllocationHandle,
    ty: u32,
    length: u32,
    field_bytes: usize,
  ) -> *mut WasmGcObject;
  fn cppgc__WasmMember__CONSTRUCT(slot: *mut u32);
  fn cppgc__WasmMember__DESTRUCT(slot: *mut u32);
  fn cppgc__WasmMember__Assign(slot: *mut u32, object: *mut WasmGcObject);
  fn cppgc__WasmMember__Get(slot: *const u32) -> *mut WasmGcObject;
  fn cppgc__Visitor__Trace__WasmMember(visitor: *mut Visitor, slot: *const u32);
  fn cppgc__HostMember__Assign(slot: *mut u32, object: *mut RustObj);
  fn cppgc__HostMember__Get(slot: *const u32) -> *mut RustObj;
  fn cppgc__TableMember__CONSTRUCT(slot: *mut u32);
  fn cppgc__TableMember__DESTRUCT(slot: *mut u32);
  fn cppgc__TableMember__Assign(slot: *mut u32, table: *mut WeakObjectTable);
  fn cppgc__Visitor__Trace__TableMember(
    visitor: *mut Visitor,
    slot: *const u32,
  );
  fn cppgc__WeakObjectTable__New(
    handle: *mut AllocationHandle,
  ) -> *mut WeakObjectTable;
  fn cppgc__WeakObjectTable__AddWasm(
    table: *mut WeakObjectTable,
    object: *const WasmGcObject,
  );
  fn cppgc__WeakObjectTable__AddHost(
    table: *mut WeakObjectTable,
    object: *const RustObj,
  );
  fn cppgc__WeakObjectTable__Enter(table: *mut WeakObjectTable);
  fn cppgc__WeakObjectTable__Exit(table: *mut WeakObjectTable);
  fn cppgc__WeakObjectTable__Size(table: *const WeakObjectTable) -> usize;
}

const _: () = assert!(crate::binding::cppgc__Member_SIZE == 4);

#[repr(u8)]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RefKind {
  NoRef = 0,
  /// A `Member` to a wasm object.
  Ref = 1,
  /// An i31 or a `Member` to a wasm object.
  AnyRef = 2,
  /// A `Member` to a Rust GC object (`GarbageCollected`).
  HostRef = 3,
}

/// A reference field of a struct type, at `offset` in the fields.
#[repr(C)]
pub struct RefField {
  pub offset: u32,
  pub kind: RefKind,
}

/// One type of the type table: a struct lists its reference fields
/// (`elem_size` 0); an array has the size and kind of its elements.
#[repr(C)]
pub struct WasmGcType {
  pub refs: *const RefField,
  pub ref_count: usize,
  pub elem_size: u32,
  pub elem_kind: RefKind,
}

// SAFETY: plain data that points to `'static` data (`set_types`).
unsafe impl Sync for WasmGcType {}

/// Sets the type table that every `WasmGcObject`'s type index refers to.
///
/// # Safety
///
/// Before any object is allocated, and only once: objects already made
/// keep their indices, and marker threads read the table. Every offset in
/// it is within the fields of the objects of that type, 4-aligned.
pub unsafe fn set_types(types: &'static [WasmGcType]) {
  // SAFETY: `'static`, so markers may read it for as long as they run.
  unsafe { cppgc__WasmGc__SetTypes(types.as_ptr()) }
}

/// A wasm struct or array (C++'s `WasmGcObject`).
#[repr(C)]
pub struct WasmGcObject(Opaque);

/// Where the fields start, from the object's address.
pub const FIELDS_OFFSET: usize = 8;

impl WasmGcObject {
  /// A new object of type `ty` with `field_bytes` of zeroed fields (a null
  /// reference is zero).
  ///
  /// # Safety
  ///
  /// `set_types` was called and `ty` is in the table with fields that fit
  /// in `field_bytes`. As for any new GC object, the pointer stays on the
  /// stack until it is stored in a reference or a root.
  pub unsafe fn new(
    handle: &AllocationHandle,
    ty: u32,
    length: u32,
    field_bytes: usize,
  ) -> NonNull<WasmGcObject> {
    debug_assert_eq!(
      unsafe { cppgc__WasmGcObject__FIELDS_OFFSET() },
      FIELDS_OFFSET
    );
    // SAFETY: per the caller.
    let object = unsafe {
      cppgc__WasmGcObject__New(
        handle as *const AllocationHandle as *mut _,
        ty,
        length,
        field_bytes,
      )
    };
    NonNull::new(object).unwrap()
  }

  /// The type index (the header's first word).
  ///
  /// # Safety
  ///
  /// `object` is alive.
  pub unsafe fn ty(object: NonNull<WasmGcObject>) -> u32 {
    // SAFETY: the header is two `uint32_t`s at the object's start.
    unsafe { object.cast::<u32>().read() }
  }

  /// # Safety
  ///
  /// `object` is alive.
  pub unsafe fn length(object: NonNull<WasmGcObject>) -> u32 {
    // SAFETY: as in `ty`.
    unsafe { object.cast::<u32>().add(1).read() }
  }

  /// The start of the fields, 8-aligned.
  pub fn fields(object: NonNull<WasmGcObject>) -> *mut u8 {
    object.as_ptr().cast::<u8>().wrapping_add(FIELDS_OFFSET)
  }
}

// Reference fields of wasm objects. Common contract (`# Safety` of each):
// on the heap's thread; `slot` is a field of that kind of a live wasm
// object; a target is alive, on the same heap.

/// A `Ref` field, or an `AnyRef` field set to an object or null.
///
/// # Safety
///
/// See above.
pub unsafe fn set_ref(slot: *mut u8, object: Option<NonNull<WasmGcObject>>) {
  let object = object.map_or(std::ptr::null_mut(), NonNull::as_ptr);
  // SAFETY: see above.
  unsafe { cppgc__WasmMember__Assign(slot.cast(), object) }
}

/// A `Ref` field (or an `AnyRef` field known not to hold an i31).
///
/// # Safety
///
/// See above.
pub unsafe fn get_ref(slot: *const u8) -> Option<NonNull<WasmGcObject>> {
  // SAFETY: see above.
  NonNull::new(unsafe { cppgc__WasmMember__Get(slot.cast()) })
}

/// A `HostRef` field.
///
/// # Safety
///
/// See above.
pub unsafe fn set_host_ref<T: GarbageCollected>(
  slot: *mut u8,
  object: Option<&impl GetRustObj<T>>,
) {
  let object = object.map_or(std::ptr::null_mut(), |o| o.get_rust_obj());
  // SAFETY: see above.
  unsafe { cppgc__HostMember__Assign(slot.cast(), object) }
}

/// A `HostRef` field's target, as a raw `RustObj` pointer (compare it with
/// `GetRustObj::get_rust_obj`).
///
/// # Safety
///
/// See above.
pub unsafe fn get_host_ref(slot: *const u8) -> *mut RustObj {
  // SAFETY: see above.
  unsafe { cppgc__HostMember__Get(slot.cast()) }
}

/// An `AnyRef` field's value.
pub enum AnyRef {
  Null,
  I31(i32),
  Object(NonNull<WasmGcObject>),
}

/// # Safety
///
/// See above.
pub unsafe fn get_any(slot: *const u8) -> AnyRef {
  // SAFETY: a 4-aligned field that every thread accesses atomically.
  let bits = unsafe { &*(slot as *const AtomicU32) }.load(Ordering::Relaxed);
  if bits & 1 == 1 {
    return AnyRef::I31((bits as i32) >> 1);
  }
  // SAFETY: not an i31, so a `Member`; see above.
  match unsafe { get_ref(slot) } {
    Some(object) => AnyRef::Object(object),
    None => AnyRef::Null,
  }
}

/// `anyref` := `ref.i31 value`: an atomic store with no barrier (not a
/// pointer, and cppgc's insertion barrier needs none for the overwritten
/// one).
///
/// # Safety
///
/// See above.
pub unsafe fn set_i31(slot: *mut u8, value: i32) {
  let bits = ((value as u32) << 1) | 1;
  // SAFETY: as in `get_any`.
  unsafe { &*(slot as *const AtomicU32) }.store(bits, Ordering::Relaxed);
}

macro_rules! cxx_member_slot {
  (
    $(#[$doc:meta])*
    $name:ident, $target:ty,
    construct: $construct:ident, destruct: $destruct:ident,
    assign: $assign:ident, trace: $trace:ident $(,)?
  ) => {
    $(#[$doc])*
    ///
    /// Like `MemberSlot`, it is `Sync` and writes through `&self`, so a
    /// marker may trace it while the heap's thread writes it.
    pub struct $name {
      storage: UnsafeCell<u32>,
    }

    // SAFETY: as `MemberSlot`'s: every access is a C++ call through a raw
    // pointer; only `trace` runs off the heap's thread, and it loads
    // atomically.
    unsafe impl Sync for $name {}

    impl $name {
      pub fn empty() -> Self {
        let slot = Self {
          storage: UnsafeCell::new(0),
        };
        // SAFETY: constructs the `Member` in place; it holds no address of
        // its own, so moving `slot` afterwards is fine.
        unsafe { $construct(slot.storage.get()) };
        slot
      }

      /// # Safety
      ///
      /// On the heap's thread; `target` is null or alive on the same heap
      /// as the object this slot is a field of.
      pub unsafe fn assign(&self, target: *mut $target) {
        // SAFETY: a live slot; per the caller.
        unsafe { $assign(self.storage.get(), target) }
      }

      /// # Safety
      ///
      /// Only from the `trace` of the GC object this slot is a field of.
      pub unsafe fn trace(&self, visitor: &mut Visitor) {
        // SAFETY: a live slot.
        unsafe { $trace(visitor, self.storage.get()) }
      }
    }

    impl Drop for $name {
      fn drop(&mut self) {
        // SAFETY: a live slot, destroyed once.
        unsafe { $destruct(self.storage.get()) }
      }
    }
  };
}

cxx_member_slot! {
  /// A strong `Member` to a wasm object, as a field of a Rust GC object.
  WasmMemberSlot, WasmGcObject,
  construct: cppgc__WasmMember__CONSTRUCT, destruct: cppgc__WasmMember__DESTRUCT,
  assign: cppgc__WasmMember__Assign, trace: cppgc__Visitor__Trace__WasmMember,
}

impl WasmMemberSlot {
  /// # Safety
  ///
  /// On the heap's thread.
  pub unsafe fn get(&self) -> Option<NonNull<WasmGcObject>> {
    // SAFETY: a live slot.
    NonNull::new(unsafe { cppgc__WasmMember__Get(self.storage.get()) })
  }
}

cxx_member_slot! {
  /// A strong `Member` to a `WeakObjectTable`, as a field of a Rust GC
  /// object (a Store's holder).
  WeakObjectTableSlot, WeakObjectTable,
  construct: cppgc__TableMember__CONSTRUCT, destruct: cppgc__TableMember__DESTRUCT,
  assign: cppgc__TableMember__Assign, trace: cppgc__Visitor__Trace__TableMember,
}

/// C++'s `WeakObjectTable` (see `src/binding.cc`), kept alive by a
/// `WeakObjectTableSlot`.
#[repr(C)]
pub struct WeakObjectTable(Opaque);

impl WeakObjectTable {
  /// # Safety
  ///
  /// As for any new GC object: the pointer is stored in a slot or a root
  /// before the next GC can finish.
  pub unsafe fn new(handle: &AllocationHandle) -> NonNull<WeakObjectTable> {
    // SAFETY: per the caller.
    let table = unsafe {
      cppgc__WeakObjectTable__New(handle as *const AllocationHandle as *mut _)
    };
    NonNull::new(table).unwrap()
  }

  // Common contract: on the heap's thread; `table` and the object added are
  // alive, on one heap.

  /// Records a wasm object the Store allocated.
  ///
  /// # Safety
  ///
  /// See above.
  pub unsafe fn add_wasm(
    table: NonNull<WeakObjectTable>,
    object: NonNull<WasmGcObject>,
  ) {
    // SAFETY: see above.
    unsafe { cppgc__WeakObjectTable__AddWasm(table.as_ptr(), object.as_ptr()) }
  }

  /// Records a host object handed to the Store (a DOM node).
  ///
  /// # Safety
  ///
  /// See above.
  pub unsafe fn add_host<T: GarbageCollected>(
    table: NonNull<WeakObjectTable>,
    object: &impl GetRustObj<T>,
  ) {
    // SAFETY: see above.
    unsafe {
      cppgc__WeakObjectTable__AddHost(table.as_ptr(), object.get_rust_obj())
    }
  }

  /// The Store starts running (nests). `table` is kept alive until the
  /// matching `exit`.
  ///
  /// # Safety
  ///
  /// See above.
  pub unsafe fn enter(table: NonNull<WeakObjectTable>) {
    // SAFETY: see above.
    unsafe { cppgc__WeakObjectTable__Enter(table.as_ptr()) }
  }

  /// # Safety
  ///
  /// After a matching `enter`; see above.
  pub unsafe fn exit(table: NonNull<WeakObjectTable>) {
    // SAFETY: see above.
    unsafe { cppgc__WeakObjectTable__Exit(table.as_ptr()) }
  }

  /// # Safety
  ///
  /// See above.
  pub unsafe fn len(table: NonNull<WeakObjectTable>) -> usize {
    // SAFETY: see above.
    unsafe { cppgc__WeakObjectTable__Size(table.as_ptr()) }
  }
}
