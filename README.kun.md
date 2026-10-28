# rusty_v8 (kun's fork)

| | |
| --- | --- |
| Upstream | https://github.com/denoland/rusty_v8 (remote `upstream`; fetch only, its push URL is disabled) |
| Fork | https://github.com/wusyong/rusty_v8 (remote `origin`) |
| Branch | `kun`, from upstream's tag `v152.2.0` (`2768994`), the `v8` crate version kun uses |
| License | MIT (`LICENSE`); V8 and the other submodules under their own licenses |
| Used by | kun's `bindings` and `dom`, by path, with `v8_enable_pointer_compression`; kun links the prebuilt library this fork's `kun-release` workflow attaches to its GitHub Releases |

## Why a fork

kun wants to build V8 from source instead of using the prebuilt static
library:

- **Pointer compression on every platform**: only then does V8 check the
  `Object::wrap` tag on `unwrap`. The prebuilt libraries with pointer
  compression exist for macOS and Linux x86-64 only, not Windows.
- **Tools the prebuilt can't give**: TSan, cppgc's marking verifier and
  debug builds with `DCHECK`s, for the GC code kun's `dom` crate builds on.
- **The V8 sandbox**, and patches of our own: the GC changes below, which
  kun's `crates/dom/soundness.md` relies on (its last section lists them
  as candidates to send upstream).

Building from source alone doesn't need a fork (`V8_FROM_SOURCE=1` with the
crate's features); the fork is for the patches.

## Status

kun builds V8 from this fork. macOS,
2026-10-02, Apple M4: built with `v8_enable_pointer_compression` in 23
minutes; kun's tests and `scripts/test.py matrix` pass against it, and
`unwrap` checks the wrap tag (kun's `bindings` test
`unwrap_checks_the_wrap_tag`). kun keeps a debug and a release build (see
`../rusty_v8_artifacts/README.kun.md`) and, as upstream does with
`RUSTY_V8_ARCHIVE`, links the release one unless `V8_FORCE_DEBUG=1` is set.
Until 2026-10-02 `build.rs` picked by cargo's profile instead, so kun's
tests linked the debug V8; that change is gone: against the release V8,
every test binary that starts V8 links and runs measurably faster, with the
same results. Windows, 2026-10-03: the release build links and
`cargo build` of kun passes (needs `v8_enable_partition_alloc = false`,
below); rusty_v8 never builds a debug V8 on Windows (`build.rs`), so
`V8_FORCE_DEBUG` has no effect there. Linux x86-64, 2026-10-03, Fedora 44:
both profiles built against Chromium's sysroot (`use_sysroot = true`,
below), release in 11 minutes and debug in 14; `bindings`' tests pass
against each, `unwrap_checks_the_wrap_tag` included. A Linux build needs
the sysroot fetched once per checkout and bindgen pointed at a Clang 21.1
or newer (`LIBCLANG_PATH`); the README next to the artifacts has both
commands.

Since 2026-10-04 kun no longer builds V8 itself by default: it downloads
the prebuilt library from this fork's GitHub Releases (see "Releases"
below). Building from source is only for working on the fork; kun's
`python3 scripts/ci.py v8` does it and says how to link the result.

## Releases

`.github/workflows/kun-release.yml` (ours; upstream's `ci.yml` only
publishes from `denoland/rusty_v8`) runs when a `kun-v*` tag is pushed. It
builds V8 from source with `v8_enable_pointer_compression` and attaches the
static library (gzipped) and the generated binding to that tag's release,
under the names `build.rs` looks up:

| Target | Profiles |
| --- | --- |
| `x86_64-pc-windows-msvc` | release (`build.rs` never uses a debug V8 on Windows) |
| `aarch64-apple-darwin` | release, debug |
| `x86_64-unknown-linux-gnu` | release, debug |

kun points `RUSTY_V8_MIRROR` at
`https://github.com/wusyong/rusty_v8/releases/download` and
`RUSTY_V8_MIRROR_TAG` at the tag (kun's `.cargo/config.toml`), and checks
this repository out at the same tag: the Rust side and the library must
come from the same commit, or the fork's own C functions don't link.

Tags are `kun-v<v8 crate version>-<n>`, e.g. `kun-v152.2.0-1`, with `n`
counting the releases of the fork on that version. To release:

1. Commit to `kun` and push it.
2. `git tag kun-v152.2.0-<n+1> && git push origin kun-v152.2.0-<n+1>`.
3. Wait for the workflow (a few hours for a new V8; Windows is the
   slowest), then move kun's `RUSTY_V8_MIRROR_TAG` to the new tag.

The workflow compiles V8 through sccache and keeps the cache between runs
(`actions/cache`, one per target and profile, keyed by the submodules and
the glue): a release that doesn't move the `v8` submodule recompiles
little besides the glue, while a new V8 compiles nearly everything again.
GitHub keeps 10 GB of caches per repository and drops the least recently
used beyond that.

Running the workflow by hand (`workflow_dispatch`) builds without
releasing; the files are kept as the run's artifacts.

## Local modifications

Code changes are marked with `kun:` comments.

- **Callback info read inline** (`src/function.rs`, 2026-10-27): a
  function callback's isolate, receiver, argument count, arguments and
  return value slot are read from V8's `FunctionCallbackInfo` slots
  (`values_`) directly, as V8's header does inline, and with pointer
  compression `ReturnValue`'s setters for objects, Smis, booleans, `null`,
  `undefined` and the empty string write the slot's lower 32 bits as
  `ReturnValue::SetInternal` does with static roots (the roots' values
  from `include/v8-internal.h`), in one relaxed atomic store of the whole
  slot (2026-10-28): LLVM narrows a plain store of it to 32 bits, which the
  builtin's 64-bit load right after the callback can't forward from (a
  stall of about 2.3 ns per call on x86-64). Upstream makes a C++ call for each, which
  kun measured as about a fifth of a cheap DOM getter. The slot indices and
  the roots are V8's, so a V8 update must check them against
  `include/v8-function-callback.h` and `include/v8-internal.h`: debug builds
  also make V8's own call and assert both agree, at every callback kun's
  tests run. Rust only: the prebuilt V8 is unchanged.
- **`ReturnValue::set_global`** (`src/function.rs`, 2026-10-28): a global's
  object written into the return value slot from the global's own slot, as
  V8's `ReturnValue::Set(const Global<S>&)` does, with no local handle made
  for it (`Local::new` is a C++ call). The isolate is checked as
  `Global::open` checks it; debug builds also make V8's own call and
  assert both agree (as the other inline setters). Rust only.
- **`v8::kun::one_byte_chars`** (`src/kun.rs`, 2026-10-28): a sequential
  one-byte string's characters (or a thin string's, forwarding to one) read
  in place, as V8's header reads a string's fields: the map's instance type
  (`kFirstNonstringType`, `kStringRepresentationMask`,
  `kStringEncodingMask`, `kSeqStringTag | kOneByteStringTag`,
  `kThinStringTag`), the length after the map and the hash, the characters
  (or a thin string's forward) after the length
  (`Internals::kStringResourceOffset`). `None` for another representation.
  `unsafe`: the slice lives while no GC runs. kun reads its DOM members'
  string arguments with it, where `ValueView` cost about 540 instructions an
  argument. A V8 update must check the constants against
  `include/v8-internal.h`, `src/objects/string.h` and
  `src/objects/instance-type.h`; debug builds check every read against
  `ValueView`'s. Rust only.
- **Isolate data read inline** (`src/isolate.rs`, 2026-10-27):
  `get_data_internal` (and so the annex every scope looks up, and
  `get_data`) reads V8's isolate data slots at
  `Internals::kIsolateEmbedderDataOffset` (688 bytes into the isolate on
  64-bit, summed from `include/v8-internal.h`'s layout constants), as V8's
  inline `Isolate::GetData` does, instead of a C++ call. A V8 update must
  check the offset; debug builds also call V8's and assert both agree.
- **New objects are initialized in the constructor** (`src/support.h`,
  `src/binding.cc`, `src/cppgc.rs`): `RustObj`'s constructor takes a
  `RustObjInit` callback, and `make_garbage_collected` passes
  `init_rust_obj::<T>`, which writes the value and the `dynamic` pointer.
  Upstream writes them after allocation returns, after cppgc's "fully
  constructed" release store, so a concurrent marker could read them
  without synchronizing. kun's `bindings` build script checks that this is
  still in place.
- **GC slots sound under concurrent marking** (`src/cppgc/slot.rs`):
  `v8::cppgc::{MemberSlot, WeakMemberSlot, TracedSlot}`, next to upstream's
  `Member`, `WeakMember` and `TracedReference`, which are unchanged. The
  slots are aligned to their size and write through `&self` with raw
  pointers, so tracing them from a marker thread while the heap's thread
  writes them is not a Rust aliasing violation. kun's `dom` uses only
  these (its `clippy.toml` disallows upstream's types).
- **Test-only GC control** (`src/cppgc/testing.rs`, C++ in
  `src/binding.cc`): `v8::cppgc::testing` wraps cppgc's
  `StandaloneTestingHeap`, step-by-step marking on a `DetachedHeap`, a heap
  that can't be attached to an isolate.
- **Standalone cppgc heap** (`src/cppgc/standalone.rs`, `src/cppgc.rs`, C++
  in `src/binding.cc`; experimental, for wasm scripts with no isolate):
  `v8::cppgc::standalone::StandaloneHeap` is a `cppgc::Heap`, not a
  `v8::CppHeap`, for running with no isolate. It schedules its own GCs and
  posts them as non-nestable foreground tasks to a `PlatformImpl` (the
  callbacks `CustomPlatform` uses, with a null isolate), and never scans
  the stack (`kNoConservativeStackScan`). Objects go on any heap's
  `AllocationHandle` with `make_garbage_collected_on`
  (`cppgc__make_garbage_collectable_on`; upstream's
  `make_garbage_collected` shares its Rust code but still calls its own C
  function). Dropping the heap runs a precise GC before destroying it
  (2026-10-06): `cppgc::Heap`'s destructor finishes a running GC but
  finalizes none of the objects left, so without it every live object's
  finalizer (a Rust `Drop`) was skipped. `is_marking` and `marking_step`
  (2026-10-06; `cppgc__StandaloneHeap__MarkingStep`, which includes
  cppgc's internal `heap.h`) let the embedder post a GC's marking steps
  after the first: cppgc's `IncrementalMarkingTask` never clears its
  handle after it ran, so `MarkerBase::ScheduleIncrementalMarkingTask`
  never posts another, and a GC whose first step doesn't finish marking
  waits for allocation to reach the atomic GC's limit. A step does what
  that task does, and finalizes with the config the heap started the GC
  with (`Heap::FinalizeIncrementalGarbageCollectionIfRunning`; the task's
  `FinalizeIncrementalGarbageCollectionIfNeeded` is protected). Upstream
  V8 has the same code; a `CppHeap` doesn't notice, since V8's
  `IncrementalMarkingJob` drives its marking. kun's `bindings` build
  script checks the cppgc code this relies on.
- **wasm GC objects and a weak table** (`src/cppgc/wasm_gc.rs`, C++ in
  `src/binding.cc`; only with pointer compression; experimental, for wasm
  GC on cppgc): `WasmGcObject` is a C++ GC object with a type
  index, a length and its fields, which it traces by a type table the
  embedder sets (`set_types`). A reference field is a 32-bit `Member` to a
  wasm object, to a `RustObj` (a host object such as a DOM node), or an
  i31-or-wasm-object, the reason the objects are 16-aligned. Each `Member`
  names its target's class: cppgc's mixin path (the trace found from the
  object's header) only works for mixin classes. `WasmMemberSlot` and
  `WeakObjectTableSlot` are such `Member`s as fields of Rust GC objects.
  `WeakObjectTable` holds the objects a wasm Store allocated or was
  handed: weakly (a weak callback prunes them) while the Store isn't
  running, strongly while it is, with a Steele barrier when the Store
  starts running after the table was traced in the same GC and a Dijkstra
  barrier for objects added while it runs. cppgc's young generation must
  stay off (it is compiled in, off at run time): its barrier records slots
  to read back as pointers, and an i31-or-object field may hold an i31.
- **`Object::is_wrapping`** (`src/object.rs`, `src/binding.cc`): whether
  an API wrapper wraps anything, whatever the tag (V8's
  `kAnyCppHeapPointer` range). `unwrap` only sees objects wrapped with the
  tag it is given, so kun's `dom` uses this to make sure a new wrapper
  wraps nothing yet.
- **A `Weak` dropped during the isolate's teardown keeps its data**
  (`src/handle.rs`, `Weak`'s `Drop`; 2026-10-08, found by kun's
  `scripts/test.py fuzz` under `--stress-compaction`): `OwnedIsolate::drop`
  disposes the isolate's handle, then runs the guaranteed finalizers, then
  V8's own teardown. A context with slots has an annex freed by such a
  finalizer, and the annex holds the `Weak<Context>` that watches the
  context (and the slots may hold `Weak`s of their own). Dropped there, a
  `Weak` sees the disposed handle (`get_pointer` returns `None`), so it
  neither resets its V8 handle nor keeps its `WeakData`, and a GC in V8's
  teardown called the first pass callback with the freed `WeakData`: an
  access violation, or V8's `Check failed: node->IsInUse()`. A `Weak` whose
  pointer is still set now leaks its `WeakData` in that case, as it already
  does while a second pass is pending, and marks it dropped, so a second
  pass frees it. The cost is a `WeakData` per weak handle still registered
  when an isolate goes, if V8 never calls it back. Upstream's `v152.2.0`
  has the same code. Rust only: the prebuilt library doesn't change. kun's
  `bindings` test `isolate_teardown` drops an isolate whose context has a
  slot under `--stress-compaction`; it crashes without this.
  Not reported upstream yet; kun's `crates/dom/soundness.md` lists it with
  the other candidates.
- **A `ValueSerializer` delegate that refused by throwing isn't asked
  again** (`src/binding.cc`,
  `v8__ValueSerializer__Delegate::GetSharedArrayBufferId`; 2026-10-08,
  found by kun's `scripts/test.py matrix`): upstream turns a `None` from
  the Rust delegate into a call to
  `v8::ValueSerializer::Delegate::GetSharedArrayBufferId`, V8's own
  delegate, which throws a `DataCloneError`. A delegate that refused by
  throwing — what Blink's `V8ScriptValueSerializer` does, and what kun's
  `dom_js` does for a `SharedArrayBuffer` in `history.pushState` — already
  has its exception pending, so that is a second throw on top of it, which
  `Isolate::Throw` only tolerates for a termination exception: a release
  V8 keeps the first exception, a debug V8 fails the check
  (`isolate.cc`'s `DCHECK(IsTerminationException(exception()))`).
  Forwarding now happens only when nothing is pending, so a delegate that
  just doesn't implement it still gets V8's own error. The wasm
  counterpart, `GetWasmModuleTransferId`, returns `Nothing` for `None` and
  never had this. Upstream's `v152.2.0` has the same code. C++: the
  prebuilt library changes, so this needs a new release. Not reported
  upstream yet; kun's `crates/dom/soundness.md` lists it with the other
  candidates.
- **No warnings under kun's Rust 1.99** (kun builds this crate with its own
  toolchain; this repo's `rust-toolchain.toml` still pins upstream's
  1.91.0):
  - `src/isolate.rs`: `#[allow(deprecated)]` on `fetch_update`, which 1.99
    deprecates for `try_update`; `try_update` is still unstable in 1.91, so
    renaming it would break this repo's own build.
  - `Cargo.toml`'s `[[test]] build` points at `tests/build.rs`, which
    includes `build.rs` as a module (`#[path]`). Pointing at `build.rs`
    itself put the file in two targets (the build script and the test),
    which Cargo 1.99 warns about. Its unit tests still run:
    `cargo test --features v8_enable_pointer_compression --test build`.
- **`Isolate::request_garbage_collection_for_testing_with_stack_state`**
  (`src/isolate.rs`, `src/binding.cc`): V8's
  `RequestGarbageCollectionForTesting(type, stack_state)` overload, which
  upstream doesn't bind. The one-argument version scans the stack
  conservatively, so a stale pointer a returned call left there can keep
  garbage alive, depending on the platform's frame layout; kun's
  `bindings` test `cppgc_traced` failed that way on Windows x64. Its tests
  GC with `NoHeapPointers` instead.
- **No PartitionAlloc** (`.gn`, `v8_enable_partition_alloc = false`): V8
  turns it on for non-embedder builds with a shared pointer compression
  cage (`v8/BUILD.gn`), meant for d8, so it came with
  `v8_enable_pointer_compression`. Its allocator shim replaces the
  process's malloc; on Windows the static shim's `malloc`/`free` clash with
  the UCRT's when kun links C/C++ code that calls them (Skia, aws-lc), and
  `kun.exe` fails to link with LNK2005. Without it V8 uses the system
  malloc, like upstream's prebuilt libraries. Its only other use outside
  d8 is `DefaultPlatform::GetZeroSegmentSize`, now 0: with the V8 sandbox,
  V8 then reserves the first 4GB itself instead of relying on
  PartitionAlloc having done so.
- **Chromium's sysroot on Linux** (`.gn`, `use_sysroot = true`): upstream
  builds against the host's headers, which makes the artifact's glibc floor
  whatever the build machine has -- on Fedora 44 the library wants
  `__isoc23_strtol` and the other C23 `strtol` symbols, added in glibc 2.38,
  so RHEL 9, Ubuntu 22.04 and Debian 12 can't link it. Chromium's Debian
  bullseye sysroot pins the floor at glibc 2.31 wherever the build runs,
  which the artifacts kun hands to other people need. It also brings glib's
  pkg-config files: Chromium's Linux compiler config asks pkg-config for
  glib while parsing its build files, even though no V8 target depends on
  that config and no V8 source reads `USE_GLIB`, so a host without glib's
  development package used to fail `gn gen`. A native x86-64 build fetches
  the sysroot once (`install-sysroot.py --arch=amd64` under
  `build/linux/sysroot_scripts`); `build.rs` does it on its own only for
  cross builds, and gn says so if it is missing.
- **V8 built with Perfetto** (`.gn`, `v8_use_perfetto = true`; submodules
  `third_party/perfetto`, `third_party/protobuf`, `third_party/zlib` and
  `tools/protoc_wrapper`, each at the commit `v8/DEPS` names; 2026-10-16):
  as in Chromium, where `v8/gni/v8.gni` turns it on whenever V8 is built
  with Chromium. Without it, V8's `TRACE_EVENT(...)` and `TRACE_GC*` --
  its GC phases, compiling, parsing, nearly all of its events -- compile to
  nothing, and only about a dozen older `TRACE_EVENT0` ones (mostly the
  snapshot's) reach the platform's `TracingController`; kun wants a trace
  with V8's events as Chrome's has them. `v8_use_perfetto_json_export` is
  off (it would build Perfetto's trace processor in to write JSON; V8
  writes Perfetto's protobuf instead), and so are `enable_js_protobuf` (its
  rules import Chromium's closure compiler) and `enable_perfetto_zstd`
  (zstd isn't in `v8/DEPS`; it only compresses whole traces). V8's
  `v8_libperfetto` still links the part of the trace processor it lists,
  which wants zlib. Linux x86-64, 2026-10-16: the release library grew from
  198.8 MB to 217.0 MB. With no tracing session nothing records: each
  event's call site checks its category's state, as in Chrome.
- **A Perfetto trace of the process** (`src/kun_trace.cc`, `src/kun_trace.rs`,
  `src/lib.rs`, `BUILD.gn`; 2026-10-16): `v8::kun_trace` starts Perfetto's
  in-process backend and one session that streams into a file (a 64 MB
  ring buffer in kun's use, written out every second; Perfetto's own
  messages only for errors), with the categories the embedder asks for,
  and gives the embedder track events of its own (category `kun`, names
  given at run time) on the thread's track, next to V8's. As Chromium's
  tracing does for Chrome, for one process. libplatform's
  `TracingController` (`InitializeForPerfetto`) isn't used: its buffer is a
  fixed 4 MB ring that silently drops the start of a longer trace. Started
  before V8's platform is initialized: V8 registers its events only if
  Perfetto is up by then. The memory the threads write into before the
  service copies their events (`shmem_size_hint_kb`) is Perfetto's largest,
  32 MB (2026-10-25): with the SDK's 256 KB, replaying a recording of the
  DOM's heap lost some of the main thread's events (cppgc's marking steps
  on allocation, thousands a second), and the track event data source drops
  rather than waits.
- **User Timing in the trace** (`src/kun_trace.cc`, `src/kun_trace.rs`;
  2026-10-18): `v8::kun_trace::mark` and `measure` write the page's
  `performance.mark()` and `measure()` as Chrome's trace has them, under
  Blink's category `blink.user_timing`: a mark is an instant on the
  thread's track, a measure a slice on a track of its own under the
  thread's (so that measures, even of one name, overlap freely; its id is
  a count spread by splitmix64, since a track's uuid is its id xored with
  its parent's and the threads' tracks are a thread id apart), both at the entry's times rather than the call's. The
  embedder's clock needn't be Perfetto's: times are passed as how long
  before now (signed, for times in the future), and taken from Perfetto's
  now. Checked against Chrome for Testing 152's trace of the same calls;
  no arguments on the events (Chrome's carry `startTime`, `callTime`).
- **cppgc's reads in Rust** (`src/cppgc/slot.rs`, `src/cppgc.rs`,
  `src/binding.cc`, `src/binding.hpp`; 2026-10-16): `MemberSlot` and
  `WeakMemberSlot` decompress their pointer in Rust (sign-extended,
  shifted by `cppgc__POINTER_COMPRESSION_SHIFT`, masked with the cage base
  `cppgc__CageBase()` gives once), and `Persistent` and `WeakPersistent`
  read `PersistentBase`'s raw pointer themselves, instead of a call into C++
  for each read: C++ inlines both (`Member::Get`, `Persistent::Get`), and in
  kun these calls were about a tenth of a DOM-heavy page's time. A debug
  build checks every such read against C++'s. Writes (the write barrier),
  and making and dropping a `Persistent` (its node holds its address), stay
  calls.

- **`UnsafePtr::from_value`** (`src/cppgc.rs`; 2026-10-17): the object a
  `&T` read from the heap is the value of, computed back from its address
  (`RustObjConcrete` is `#[repr(C)]`; the inverse of `as_ref`), so kun can
  root a node it reached through an edge without looking it up in a table.

- **`FunctionTemplate::set_accept_any_receiver`** (`src/template.rs`,
  `src/binding.cc`; 2026-10-21): V8's `SetAcceptAnyReceiver`, unbound
  upstream. kun's member functions set it to false, as Blink's
  `IDLMemberInstaller` does for every member that isn't `[CrossOrigin]`:
  V8 then checks that a call's receiver may be accessed, so a realm's own
  function called on another origin's window throws instead of running
  (V8's default skips the check).

- **`Template::set_lazy_data_property`** (`src/template.rs`,
  `src/binding.cc`; 2026-10-21): V8's `Template::SetLazyDataProperty`,
  unbound upstream (only `Object`'s is). kun installs a global's interface
  objects with it on the global's template, as Blink's
  `IDLMemberInstaller::InstallExposedConstructs` does on Window's instance
  template: a realm makes none of them until script reads one.
- **`ObjectTemplate::mark_as_undetectable`** (`src/template.rs`,
  `src/binding.cc`; 2026-10-24): V8's `ObjectTemplate::MarkAsUndetectable`,
  unbound upstream. kun marks `HTMLAllCollection`'s instance template with
  it, as Blink does, for `document.all`'s [[IsHTMLDDA]] (`typeof` is
  `"undefined"`, `== null`, converts to `false`); its instances are callable
  too (`set_call_as_function_handler`), which V8 requires of them.
- **`Isolate::set_is_js_api_wrapper_native_error_callback`**
  (`src/isolate.rs`, `src/binding.cc`; 2026-10-22): V8's
  `SetIsJSApiWrapperNativeErrorCallback`, unbound upstream. V8 asks it
  whether an API wrapper is a native error (`Error.isError`); kun answers
  true for a `DOMException`'s wrapper, as Blink's `V8Initializer` does.
- **`WeakPersistentSlot`** (`src/cppgc/slot.rs`, `src/cppgc.rs`,
  `src/binding.cc`, `src/binding.hpp`; 2026-10-22): a `WeakPersistent`
  built in memory the caller owns (`cppgc__WeakPersistent__CONSTRUCT_IN_PLACE`,
  `DESTRUCT_IN_PLACE`) instead of one C++ `new` each, which upstream's
  `WeakPersistent` is because the persistent region's node records the
  persistent's address and a Rust move doesn't tell C++. An empty one has
  no node, so `empty` makes it by value; `set` and `clear` take
  `Pin<&Self>`, so once set it stays put until dropped, and `index` pins an
  element of a pinned slice. Its size comes from the build
  (`cppgc__WeakPersistent_SIZE`): with cppgc's object names, on by default
  on desktop builds, it keeps a `SourceLocation` (32 bytes on x64). kun
  keeps the id table's weak persistents in chunks of these; on Windows the
  `new` and `delete` were most of the NT heap's time in Speedometer.

- **What kun's DevTools needs from the inspector** (`src/inspector.rs`,
  `src/exception.rs`, `src/binding.cc`; 2026-10-23):
  `V8InspectorClientImpl::current_time_ms` (`V8InspectorClient::currentTimeMS`,
  unbound upstream, whose default answers 0: every `console` message's and
  protocol event's time was 0), `V8Inspector::exception_revoked`
  (`exceptionRevoked`: a promise's rejection handled after
  `exception_thrown` reported it), `Message::get_script_id`
  (`GetScriptOrigin().ScriptId()`: `exception_thrown` takes the script's
  id, without which the inspector leaves the exception object out),
  `V8Inspector::context_created_with_origin` (`contextCreated` with
  `V8ContextInfo::origin`, which DevTools shows a context by; upstream's
  `context_created` leaves it empty), and `V8Inspector::reset_context_group`
  (`resetContextGroup`: a main frame's navigation clears its contexts and
  console messages at once). kun's renderer connects Chrome's DevTools to
  V8's inspector.
- **The stack as script runs, for DevTools' initiators** (`src/inspector.rs`,
  `src/binding.cc`, `src/crdtp_binding.cc`; 2026-10-24):
  `V8Inspector::capture_stack_trace` (`captureStackTrace`),
  `V8StackTrace::is_empty` and `V8StackTrace::build_inspector_object`
  (`buildInspectorObject`, serialized to CBOR; upstream's
  `crdtp::cbor_to_json` makes it JSON). As Blink's `CaptureSourceLocation`
  and `BuildInspectorObject` give a request's initiator and a frame's
  creator their stack, with the async chain V8's inspector keeps (`await`s
  included). Upstream binds only `createStackTrace`, from a `v8::StackTrace`,
  which has no async chain. C++: the prebuilt library changes. kun's
  `bindings` test `stack_traces` checks the frames and the `await` parent.
- **A heap's sizes** (`src/cppgc.rs`, `src/cppgc/standalone.rs`,
  `src/binding.cc`; 2026-10-24): `Heap::collect_statistics` and
  `StandaloneHeap::collect_statistics`, cppgc's `CollectStatistics` at
  `DetailLevel::kBrief` (committed, resident, used and pooled bytes),
  unbound upstream. Only the brief level: the detailed one finishes
  sweeping first, which would change the GC kun measures. The public
  `cppgc::Heap` has no `CollectStatistics`, so the standalone heap's goes
  through cppgc's internal `heap.h`, as `MarkingStep` does. C++: the
  prebuilt library changes. kun's `dom` measures its GC with it.
- **A standalone heap's GC count** (`src/cppgc/standalone.rs`,
  `src/binding.cc`; 2026-10-25): `StandaloneHeap::gc_count`, the GCs whose
  marking completed, counted by a `StatsCollector::AllocationObserver`
  (cppgc's internal `stats-collector.h`) the heap's platform holds,
  registered as the heap is created and unregistered before it is
  destroyed. cppgc runs most of a standalone heap's GCs inside its own
  tasks (an atomic one whole), where the embedder doesn't see them, and
  the public `HeapState` tells only whether one is marking or sweeping.
  Unbound upstream. C++: the prebuilt library changes. kun's `dom` records
  when its heap collected with it.
- **kun's own V8** (`.gitmodules`, the `v8` submodule; 2026-10-28): the
  submodule points at kun's V8 fork (https://github.com/wusyong/v8, branch
  `kun`), started from `c4ca1ecc`, the commit upstream's `v152.2.0` pins on
  `denoland/v8`'s `15.2-lkgr-denoland`. That branch is re-made by Deno's
  autoroll and force-pushed (on 2026-10-27 it no longer held `c4ca1ecc`,
  which was still fetchable by its SHA), so the pinned commit lives on a
  branch of kun's own too. The V8 fork's changes are in its own
  `README.kun.md`: an embedder heap marking with V8's major GCs
  (`include/v8-kun.h`). C++: the prebuilt library changes.
- **An embedder heap marking with V8's major GCs** (`src/kun.rs`,
  `src/binding.cc`; 2026-10-28): the glue's C functions for the V8 fork's
  `v8-kun.h`, and `v8::kun`, their Rust: an `EmbedderMarking` set on an
  isolate's `CppHeap` (V8 starts, advances and ends the embedder heap's
  marking with its major GCs; finished, unset and deleted when its
  registration drops, as Chrome finishes its GC before it detaches a
  `CppHeap`), `wrap`/`unwrap` with the tags `0x4000..=0x4fff` (the
  embedder's object's address, not a cppgc object's), `clone_object` (a
  wrapper from a boilerplate one, V8's `Object::Clone`),
  `TracedSlot::set_droppable` and a `RootsHandler` that resets the slot of
  a wrapper V8's scavenge dropped, `TracedMarker` (the embedder heap's
  threads marking its objects' `TracedReference`s into V8's worklists),
  `report_embedder_allocation`, and `post_job` (a job on V8's platform,
  `v8::JobTask`, which upstream doesn't bind from Rust). Thin: kun's
  `bindings` builds the embedder heap's rules on them (`unified_heap`,
  `gc_platform`). `TracedSlot::raw` is `pub(crate)` for it. Callbacks from
  C++ abort on a panic. Only with pointer compression (the patch tells the
  embedder's objects from cppgc's by their entries' tags). C++: the
  prebuilt library changes. kun's `bindings` tests (`unified_heap`,
  `unified_heap_marking`, `gc_platform`) run it with V8's full,
  incremental and minor GCs.
  `unwrap` reads an API wrapper's object inline (2026-10-28), as V8's
  header inlines `Object::Unwrap` (`Internals::CanHaveInternalField`,
  `internal::ReadCppHeapPointerField`): the map's instance type
  (`JS_SPECIAL_API_OBJECT_TYPE`, `FIRST_`/`LAST_JS_API_OBJECT_TYPE`), the
  `CppHeapPointerHandle` after the object's header
  (`Internals::kJSObjectHeaderSize`), the isolate's table
  (`Internals::kIsolateCppHeapPointerTableOffset`, past the embedder data
  slots, which is why `EMBEDDER_DATA_OFFSET` is `pub(crate)`) and the
  entry's tag and payload (`kExternalPointerIndexShift`,
  `kCppHeapPointerTagShift`, `kCppHeapPointerPayloadShift`); any other
  object goes through the glue's C function. A V8 update must check those
  constants (`src/kun.rs`) against `include/v8-internal.h`; debug builds
  also call the glue's and assert both agree, at every unwrap kun's tests
  make.
- **`.github/workflows/kun-release.yml`**: builds and publishes kun's
  prebuilt V8 (see "Releases").
- **`Cargo.lock`: bindgen 0.72.1 and clang-sys 1.9.1** (upstream locks
  0.72.0 and 1.8.1). Built with 0.72.0 on Windows, the generated binding
  names `RustObj`'s base `Wrappable` without its namespace, and with
  `v8_enable_pointer_compression` it came out as an empty type: the
  binding's layout check failed (`size_of::<RustObj>()` 1, C++ 8) and the
  `v8` crate didn't compile (the `kun-v152.2.0-2` release run,
  2026-10-04). 0.72.1, which kun's own lockfile had when V8 was first
  built on Windows, names it `v8_Object_Wrappable`, as 0.72.0 already does
  on the other platforms.
- `.gitignore`: `/gen/*.rs`. The build script copies the generated
  binding into `gen/` (from `RUSTY_V8_ARCHIVE`'s directory, or a download
  from `RUSTY_V8_MIRROR`); upstream only tracks `gen/.gitkeep`, so it
  showed up as untracked.

Otherwise `kun` is upstream's `v152.2.0` plus this file, with the `v8`
submodule at kun's V8 fork.

## Layout and setup

This directory sits next to kun, at `../third_party/rusty_v8` from kun's
root. Linking the prebuilt library needs only the `v8` submodule (kun's
`bindings` build script reads V8's sources):

```sh
git submodule update --init --depth 1 v8
```

Building from source needs all of them (V8, Chromium's `build`,
`buildtools`, clang and libc++, ICU...):

```sh
git submodule update --init --recursive --depth 1
```

`tools/win` is skipped by the upstream configuration; a Windows build from
source has to fetch it by hand (upstream's `README.md`, "Build V8 from
Source").

## Updating

1. `git fetch upstream tag vX.Y.Z --no-tags`, then rebase `kun` (or start a
   new branch) on that tag, matching the `v8` version kun moves to.
2. In kun's V8 fork, rebase `kun` onto the commit that tag's `v8` submodule
   pins (on `denoland/v8`'s branch for that V8 version), push it, and point
   the submodule (and `.gitmodules`'s URL, which the rebase takes back to
   upstream's) at it.
3. `git submodule update --init --recursive --depth 1`.
4. Reapply and recheck the local modifications listed above: upstream's
   changes to `make_garbage_collected`, `RustObj`, and the C functions the
   slots call (`cppgc__Member__*`, `v8__TracedReference__*`).
5. In kun: rebuild from source (`python3 scripts/ci.py v8`, linked with
   `RUSTY_V8_ARCHIVE`), then `cargo test --workspace` and
   `python scripts/test.py matrix`, and recheck the assumptions in
   `crates/dom/soundness.md` (its "需要人工審查的地方" covers a `v8`
   upgrade).
5. Release it (see "Releases", with `n` back to 1) and point kun at the new
   tag.
