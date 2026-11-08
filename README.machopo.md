# rusty_v8 (machopo's fork)

| | |
| --- | --- |
| Upstream | https://github.com/denoland/rusty_v8 (remote `upstream`; fetch only, its push URL is disabled) |
| Fork | https://github.com/wusyong/rusty_v8 (remote `origin`) |
| Branch | `machopo`, from upstream's tag `v152.2.0` (`2768994`), the `v8` crate version machopo uses |
| License | MIT (`LICENSE`); V8 and the other submodules under their own licenses |
| Used by | machopo's `bindings` and `dom`, by path, with `v8_enable_pointer_compression`; machopo links the prebuilt library this fork's `machopo-release` workflow attaches to its GitHub Releases |

## Why a fork

machopo wants to build V8 from source instead of using the prebuilt static
library:

- **Pointer compression on every platform**: only then does V8 check the
  `Object::wrap` tag on `unwrap`. The prebuilt libraries with pointer
  compression exist for macOS and Linux x86-64 only, not Windows.
- **Tools the prebuilt can't give**: TSan, cppgc's marking verifier and
  debug builds with `DCHECK`s, for the GC code machopo's `dom` crate builds on.
- **The V8 sandbox**, and patches of our own: the GC changes below, which
  machopo's `crates/bindings/soundness.md` relies on (its last section lists them
  as candidates to send upstream).

Building from source alone doesn't need a fork (`V8_FROM_SOURCE=1` with the
crate's features); the fork is for the patches.

## Status

machopo builds V8 from this fork. macOS,
2026-10-02, Apple M4: built with `v8_enable_pointer_compression` in 23
minutes; machopo's tests and `scripts/test.py matrix` pass against it, and
`unwrap` checks the wrap tag (machopo's `bindings` test
`unwrap_checks_the_wrap_tag`). machopo keeps a debug and a release build (see
`../rusty_v8_artifacts/README.machopo.md`) and, as upstream does with
`RUSTY_V8_ARCHIVE`, links the release one unless `V8_FORCE_DEBUG=1` is set.
Until 2026-10-02 `build.rs` picked by cargo's profile instead, so machopo's
tests linked the debug V8; that change is gone: against the release V8,
every test binary that starts V8 links and runs measurably faster, with the
same results. Windows, 2026-10-03: the release build links and
`cargo build` of machopo passes (needs `v8_enable_partition_alloc = false`,
below); rusty_v8 never builds a debug V8 on Windows (`build.rs`), so
`V8_FORCE_DEBUG` has no effect there. Linux x86-64, 2026-10-03, Fedora 44:
both profiles built against Chromium's sysroot (`use_sysroot = true`,
below), release in 11 minutes and debug in 14; `bindings`' tests pass
against each, `unwrap_checks_the_wrap_tag` included. A Linux build needs
the sysroot fetched once per checkout and bindgen pointed at a Clang 21.1
or newer (`LIBCLANG_PATH`); the README next to the artifacts has both
commands.

Since 2026-10-04 machopo no longer builds V8 itself by default: it downloads
the prebuilt library from this fork's GitHub Releases (see "Releases"
below). Building from source is only for working on the fork; machopo's
`python3 scripts/ci.py v8` does it and says how to link the result.

## Releases

`.github/workflows/machopo-release.yml` (ours; upstream's `ci.yml` only
publishes from `denoland/rusty_v8`) runs when a `machopo-v*` tag is pushed. It
builds V8 from source with `v8_enable_pointer_compression` and attaches the
static library (gzipped) and the generated binding to that tag's release,
under the names `build.rs` looks up:

| Target | Profiles |
| --- | --- |
| `x86_64-pc-windows-msvc` | release (`build.rs` never uses a debug V8 on Windows) |
| `aarch64-apple-darwin` | release, debug |
| `x86_64-unknown-linux-gnu` | release, debug |

machopo points `RUSTY_V8_MIRROR` at
`https://github.com/wusyong/rusty_v8/releases/download` and
`RUSTY_V8_MIRROR_TAG` at the tag (machopo's `.cargo/config.toml`), and checks
this repository out at the same tag: the Rust side and the library must
come from the same commit, or the fork's own C functions don't link.

Tags are `machopo-v<v8 crate version>-<n>`, e.g. `machopo-v152.2.0-29`, with `n`
counting the releases of the fork on that version (until
`kun-v152.2.0-28` the tags began with `kun-`, the project's earlier name). To
release:

1. Commit to `machopo` and push it.
2. `git tag machopo-v152.2.0-<n+1> && git push origin machopo-v152.2.0-<n+1>`.
3. Wait for the workflow (a few hours for a new V8; Windows is the
   slowest), then move machopo's `RUSTY_V8_MIRROR_TAG` to the new tag.

The workflow compiles V8 through sccache and keeps the cache between runs
(`actions/cache`, one per target and profile, keyed by the submodules and
the glue): a release that doesn't move the `v8` submodule recompiles
little besides the glue, while a new V8 compiles nearly everything again.
GitHub keeps 10 GB of caches per repository and drops the least recently
used beyond that.

Running the workflow by hand (`workflow_dispatch`) builds without
releasing; the files are kept as the run's artifacts.

## Local modifications

Code changes are marked with `machopo:` comments.

- **`Context::detach_global`** (`src/context.rs`, `src/binding.cc`,
  2026-11-02): V8's `Context::DetachGlobal`, for machopo's window proxy that
  outlives a navigation (HTML's `WindowProxy`, Blink's `LocalWindowProxy`):
  the realm before is detached from its global proxy, and the new one is
  made with it (`ContextOptions::global_object`, upstream's). It changes
  the glue: machopo links a V8 built from this checkout (`RUSTY_V8_ARCHIVE`)
  until a release has it.
- **Cross-origin access checks** (`src/template.rs`, `src/isolate.rs`,
  `src/scope.rs`, `src/binding.cc`, 2026-11-02): V8's
  `ObjectTemplate::SetAccessCheckCallbackAndHandler`
  (`set_access_check_callback_and_handler`, with `AccessCheckCallback`)
  and `Isolate::SetFailedAccessCheckCallbackFunction`
  (`set_failed_access_check_callback_function`, with `AccessType` and
  `FailedAccessCheckCallback`), for machopo's cross-origin `Window` and
  `Location` (HTML's cross-origin objects, Blink's `V8Window` and
  `V8Location` interceptors: a refused access goes through the handler's
  interceptors, what else is refused throws a `SecurityError`); and
  `GetIsolate` for `Local<Object>` (the thread's current isolate, as
  `Local<Context>`'s), so that the refused access's callback, given only
  the object, opens a `CallbackScope`. It changes the glue too.
- **`get_incumbent_context`** (`src/scope.rs`, `src/scope/raw.rs`,
  `src/binding.cc`, 2026-11-02): V8's `Isolate::GetIncumbentContext`, the
  realm of the most recent script on the stack (HTML's incumbent realm),
  which a message's `source` and origin are (Blink's
  `IncumbentDOMWindow`). It changes the glue too.
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
  machopo measured as about a fifth of a cheap DOM getter. The slot indices and
  the roots are V8's, so a V8 update must check them against
  `include/v8-function-callback.h` and `include/v8-internal.h`: debug builds
  also make V8's own call and assert both agree, at every callback machopo's
  tests run. Rust only: the prebuilt V8 is unchanged.
- **`ReturnValue::set_global`, `set_weak`, `set_traced`** (`src/function.rs`,
  `src/handle.rs`, `src/cppgc/slot.rs`, 2026-10-28): a global's, a weak
  global's or a `TracedSlot`'s object written into the return value slot
  from the handle's own location (a traced reference's node's object, as
  `BasicTracedReference::ptr` reads it), as V8's `ReturnValue::Set` of a
  `Global` or a `BasicTracedReference` does, with no local handle made for
  it (`Local::new`, `Weak::to_local` and `TracedSlot::get` are C++ calls).
  `set_weak` and `set_traced` write nothing and return `false` for a
  collected or empty handle. The isolate is checked as `Global::open` and
  `Weak::to_local` check it (`Weak::location`, `pub(crate)`); debug builds
  also make V8's own call and assert both agree (as the other inline
  setters). Rust only.
- **`v8::machopo::one_byte_chars`** (`src/machopo.rs`, 2026-10-28): a sequential
  one-byte string's characters (or a thin string's, forwarding to one) read
  in place, as V8's header reads a string's fields: the map's instance type
  (`kFirstNonstringType`, `kStringRepresentationMask`,
  `kStringEncodingMask`, `kSeqStringTag | kOneByteStringTag`,
  `kThinStringTag`), the length after the map and the hash, the characters
  (or a thin string's forward) after the length
  (`Internals::kStringResourceOffset`). `None` for another representation.
  `unsafe`: the slice lives while no GC runs. machopo reads its DOM members'
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
  without synchronizing. machopo's `bindings` build script checks that this is
  still in place.
- **A traced reference sound under concurrent marking** (`src/cppgc/slot.rs`):
  `v8::cppgc::TracedSlot`, next to upstream's `TracedReference`, which is
  unchanged. It is aligned to its size and writes through `&self` with raw
  pointers, so tracing it from a marker thread while the heap's thread
  writes it is not a Rust aliasing violation. machopo's `bindings` keeps a JS
  value's edge from its `gc` heap in one (2026-11-06: the `MemberSlot`,
  `WeakMemberSlot` and `WeakPersistentSlot` beside it, the standalone heap,
  test-only GC control, wasm GC objects and the heap statistics went with
  the DOM that used them).
- **`Object::is_wrapping`** (`src/object.rs`, `src/binding.cc`): whether
  an API wrapper wraps anything, whatever the tag (V8's
  `kAnyCppHeapPointer` range). `unwrap` only sees objects wrapped with the
  tag it is given, so machopo's `dom` uses this to make sure a new wrapper
  wraps nothing yet.
- **A `Weak` dropped during the isolate's teardown keeps its data**
  (`src/handle.rs`, `Weak`'s `Drop`; 2026-10-08, found by machopo's
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
  has the same code. Rust only: the prebuilt library doesn't change. machopo's
  `bindings` test `isolate_teardown` drops an isolate whose context has a
  slot under `--stress-compaction`; it crashes without this.
  Not reported upstream yet; machopo's `crates/bindings/soundness.md` lists it with
  the other candidates.
- **A `ValueSerializer` delegate that refused by throwing isn't asked
  again** (`src/binding.cc`,
  `v8__ValueSerializer__Delegate::GetSharedArrayBufferId`; 2026-10-08,
  found by machopo's `scripts/test.py matrix`): upstream turns a `None` from
  the Rust delegate into a call to
  `v8::ValueSerializer::Delegate::GetSharedArrayBufferId`, V8's own
  delegate, which throws a `DataCloneError`. A delegate that refused by
  throwing — what Blink's `V8ScriptValueSerializer` does, and what machopo's
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
  upstream yet; machopo's `crates/bindings/soundness.md` lists it with the other
  candidates.
- **No warnings under machopo's Rust 1.99** (machopo builds this crate with its own
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
  garbage alive, depending on the platform's frame layout; machopo's
  `bindings` test `cppgc_traced` failed that way on Windows x64. Its tests
  GC with `NoHeapPointers` instead.
- **No PartitionAlloc** (`.gn`, `v8_enable_partition_alloc = false`): V8
  turns it on for non-embedder builds with a shared pointer compression
  cage (`v8/BUILD.gn`), meant for d8, so it came with
  `v8_enable_pointer_compression`. Its allocator shim replaces the
  process's malloc; on Windows the static shim's `malloc`/`free` clash with
  the UCRT's when machopo links C/C++ code that calls them (Skia, aws-lc), and
  `machopo.exe` fails to link with LNK2005. Without it V8 uses the system
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
  which the artifacts machopo hands to other people need. It also brings glib's
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
  snapshot's) reach the platform's `TracingController`; machopo wants a trace
  with V8's events as Chrome's has them. `v8_use_perfetto_json_export` is
  off (it would build Perfetto's trace processor in to write JSON; V8
  writes Perfetto's protobuf instead), and so are `enable_js_protobuf` (its
  rules import Chromium's closure compiler) and `enable_perfetto_zstd`
  (zstd isn't in `v8/DEPS`; it only compresses whole traces). V8's
  `v8_libperfetto` still links the part of the trace processor it lists,
  which wants zlib. Linux x86-64, 2026-10-16: the release library grew from
  198.8 MB to 217.0 MB. With no tracing session nothing records: each
  event's call site checks its category's state, as in Chrome.
- **A Perfetto trace of the process** (`src/machopo_trace.cc`, `src/machopo_trace.rs`,
  `src/lib.rs`, `BUILD.gn`; 2026-10-16): `v8::machopo_trace` starts Perfetto's
  in-process backend and one session that streams into a file (a 64 MB
  ring buffer in machopo's use, written out every second; Perfetto's own
  messages only for errors), with the categories the embedder asks for,
  and gives the embedder track events of its own (category `machopo`, names
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
- **User Timing in the trace** (`src/machopo_trace.cc`, `src/machopo_trace.rs`;
  2026-10-18): `v8::machopo_trace::mark` and `measure` write the page's
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
- **`FunctionTemplate::set_accept_any_receiver`** (`src/template.rs`,
  `src/binding.cc`; 2026-10-21): V8's `SetAcceptAnyReceiver`, unbound
  upstream. machopo's member functions set it to false, as Blink's
  `IDLMemberInstaller` does for every member that isn't `[CrossOrigin]`:
  V8 then checks that a call's receiver may be accessed, so a realm's own
  function called on another origin's window throws instead of running
  (V8's default skips the check).

- **`Template::set_lazy_data_property`** (`src/template.rs`,
  `src/binding.cc`; 2026-10-21): V8's `Template::SetLazyDataProperty`,
  unbound upstream (only `Object`'s is). machopo installs a global's interface
  objects with it on the global's template, as Blink's
  `IDLMemberInstaller::InstallExposedConstructs` does on Window's instance
  template: a realm makes none of them until script reads one.
- **`ObjectTemplate::mark_as_undetectable`** (`src/template.rs`,
  `src/binding.cc`; 2026-10-24): V8's `ObjectTemplate::MarkAsUndetectable`,
  unbound upstream. machopo marks `HTMLAllCollection`'s instance template with
  it, as Blink does, for `document.all`'s [[IsHTMLDDA]] (`typeof` is
  `"undefined"`, `== null`, converts to `false`); its instances are callable
  too (`set_call_as_function_handler`), which V8 requires of them.
- **`Isolate::set_is_js_api_wrapper_native_error_callback`**
  (`src/isolate.rs`, `src/binding.cc`; 2026-10-22): V8's
  `SetIsJSApiWrapperNativeErrorCallback`, unbound upstream. V8 asks it
  whether an API wrapper is a native error (`Error.isError`); machopo answers
  true for a `DOMException`'s wrapper, as Blink's `V8Initializer` does.
- **What machopo's DevTools needs from the inspector** (`src/inspector.rs`,
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
  console messages at once). machopo's renderer connects Chrome's DevTools to
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
  which has no async chain. C++: the prebuilt library changes. machopo's
  `bindings` test `stack_traces` checks the frames and the `await` parent.
- **machopo's own V8** (`.gitmodules`, the `v8` submodule; 2026-10-28): the
  submodule points at machopo's V8 fork (https://github.com/wusyong/v8, branch
  `machopo`), started from `c4ca1ecc`, the commit upstream's `v152.2.0` pins on
  `denoland/v8`'s `15.2-lkgr-denoland`. That branch is re-made by Deno's
  autoroll and force-pushed (on 2026-10-27 it no longer held `c4ca1ecc`,
  which was still fetchable by its SHA), so the pinned commit lives on a
  branch of machopo's own too. The V8 fork's changes are in its own
  `README.machopo.md`: an embedder heap marking with V8's major GCs
  (`include/v8-machopo.h`). C++: the prebuilt library changes.
- **An embedder heap marking with V8's major GCs** (`src/machopo.rs`,
  `src/binding.cc`; 2026-10-28): the glue's C functions for the V8 fork's
  `v8-machopo.h`, and `v8::machopo`, their Rust: an `EmbedderMarking` set on an
  isolate's `CppHeap` (V8 starts, advances and ends the embedder heap's
  marking with its major GCs; finished, unset and deleted when its
  registration drops, as Chrome finishes its GC before it detaches a
  `CppHeap`), `wrap`/`unwrap` with the tags `0x4000..=0x4fff` (the
  embedder's object's address, not a cppgc object's), `clone_object` (a
  wrapper from a boilerplate one, V8's `Object::Clone`),
  `function_template_with_cache` (a getter V8 reads from a private
  property of its holder once there is one, `FunctionTemplate::NewWithCache`,
  as Blink's `[CachedAccessor]`; 2026-11-07),
  `TracedSlot::set_droppable` and a `RootsHandler` that resets the slot of
  a wrapper V8's scavenge dropped (`TryResetRoot` on V8's threads in
  parallel, as Blink's `BlinkRootsHandler`, else `ResetRoot`), `TracedMarker` (the embedder heap's
  threads marking its objects' `TracedReference`s into V8's worklists),
  `report_embedder_used_size` (the embedder heap's used bytes, a total;
  a marking's `end` returns its live ones, which V8 sets its limits from),
  and `post_job` (a job on V8's platform,
  `v8::JobTask`, which upstream doesn't bind from Rust). Thin: machopo's
  `bindings` builds the embedder heap's rules on them (`unified_heap`,
  `gc_platform`). `TracedSlot::raw` is `pub(crate)` for it. Callbacks from
  C++ abort on a panic. Only with pointer compression (the patch tells the
  embedder's objects from cppgc's by their entries' tags). C++: the
  prebuilt library changes. machopo's `bindings` tests (`unified_heap`,
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
  constants (`src/machopo.rs`) against `include/v8-internal.h`; debug builds
  also call the glue's and assert both agree, at every unwrap machopo's tests
  make.
  `unwrap_receiver` (2026-11-08, `unsafe`) is the same read past the map:
  for a call's receiver that the callback's `Signature` checked, an object
  of a `FunctionTemplate`'s or a global proxy whose global is one (V8's
  `IsCompatibleReceiver`), a `JSAPIObjectWithEmbedderSlots` or a
  `JSSpecialObject` whose handle follows the header alike, so its map
  needn't be read again (V8's header's `Object::Unwrap` doesn't read it
  either). The
  handle, the entry and its tag are read as `unwrap`'s; debug builds check
  it against the glue's. Only without V8's sandbox (not built with the
  `v8_enable_sandbox` feature): with it, an object's fields are no witness
  to what it is. The instance type check was some
  28% of a `nodeType` call's callback in machopo's samples.
- **The embedder's objects in heap snapshots** (`src/machopo.rs`,
  `src/binding.cc`; 2026-11-05): V8's public `EmbedderGraph`, unbound
  upstream: `v8::machopo::add_build_embedder_graph_callback`
  (`HeapProfiler::AddBuildEmbedderGraphCallback`, removed with
  `RemoveBuildEmbedderGraphCallback` when its registration drops; a
  `BuildEmbedderGraph` trait), and `EmbedderGraph`'s `add_node`
  (`AddNode` of the glue's `RustEmbedderGraphNode`, a plain data node:
  its name, size, address, `Detachedness`, wrapper node and whether it is
  a root are copied in as it is added, so V8's questions after the
  callback need no call into Rust), `v8_node` (`V8Node`), `add_edge` and
  `add_weak_edge` (`AddEdge`, `AddWeakEdge`; a name is a `&'static CStr`,
  which V8 reads after the callback) and `add_native_size`. Nodes are
  branded with the build's invariant lifetime, so they can't outlive the
  graph nor mix with another's. The callback aborts on a panic. As Blink's
  `V8EmbedderGraphBuilder`: machopo's `bindings` adds the `gc` heap's objects
  with it, which cppgc's graph builder leaves out (the V8 fork's patch).
  C++: the prebuilt library changes. machopo's `bindings` test
  `heap_snapshot` checks the nodes, edges, wrapper merge and detachedness.
- **`.github/workflows/machopo-release.yml`**: builds and publishes machopo's
  prebuilt V8 (see "Releases").
- **`Cargo.lock`: bindgen 0.72.1 and clang-sys 1.9.1** (upstream locks
  0.72.0 and 1.8.1). Built with 0.72.0 on Windows, the generated binding
  names `RustObj`'s base `Wrappable` without its namespace, and with
  `v8_enable_pointer_compression` it came out as an empty type: the
  binding's layout check failed (`size_of::<RustObj>()` 1, C++ 8) and the
  `v8` crate didn't compile (the `kun-v152.2.0-2` release run,
  2026-10-04). 0.72.1, which machopo's own lockfile had when V8 was first
  built on Windows, names it `v8_Object_Wrappable`, as 0.72.0 already does
  on the other platforms.
- `.gitignore`: `/gen/*.rs`. The build script copies the generated
  binding into `gen/` (from `RUSTY_V8_ARCHIVE`'s directory, or a download
  from `RUSTY_V8_MIRROR`); upstream only tracks `gen/.gitkeep`, so it
  showed up as untracked.

Otherwise `machopo` is upstream's `v152.2.0` plus this file, with the `v8`
submodule at machopo's V8 fork.

## Layout and setup

This directory sits next to machopo, at `../third_party/rusty_v8` from machopo's
root. Linking the prebuilt library needs only the `v8` submodule (machopo's
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

1. `git fetch upstream tag vX.Y.Z --no-tags`, then rebase `machopo` (or start a
   new branch) on that tag, matching the `v8` version machopo moves to.
2. In machopo's V8 fork, rebase `machopo` onto the commit that tag's `v8` submodule
   pins (on `denoland/v8`'s branch for that V8 version), push it, and point
   the submodule (and `.gitmodules`'s URL, which the rebase takes back to
   upstream's) at it.
3. `git submodule update --init --recursive --depth 1`.
4. Reapply and recheck the local modifications listed above: upstream's
   changes to `make_garbage_collected`, `RustObj`, and the C functions the
   slots call (`cppgc__Member__*`, `v8__TracedReference__*`).
5. In machopo: rebuild from source (`python3 scripts/ci.py v8`, linked with
   `RUSTY_V8_ARCHIVE`), then `cargo test --workspace` and
   `python scripts/test.py matrix`, and recheck the assumptions in
   `crates/bindings/soundness.md` (its "需要人工審查的地方" covers a `v8`
   upgrade).
5. Release it (see "Releases", with `n` back to 1) and point machopo at the new
   tag.
