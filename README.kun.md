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
library (`rfcs/pre-rfcs/P3.md` in kun, section 2, "Wrapper"):

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

kun builds V8 from this fork (kun's `rfcs/pre-rfcs/P3.md`, decision 10). macOS,
2026-10-02, Apple M4: built with `v8_enable_pointer_compression` in 23
minutes; kun's tests and `scripts/ci.py matrix` pass against it, and
`unwrap` checks the wrap tag (kun's `bindings` test
`unwrap_checks_the_wrap_tag`). kun keeps a debug and a release build (see
`../rusty_v8_artifacts/README.kun.md`) and, as upstream does with
`RUSTY_V8_ARCHIVE`, links the release one unless `V8_FORCE_DEBUG=1` is set.
Until 2026-10-02 `build.rs` picked by cargo's profile instead, so kun's
tests linked the debug V8; that change is gone (kun's `rfcs/pre-rfcs/P3.md`,
decision 10, has why). Windows, 2026-10-03: the release build links and
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
3. Wait for the workflow (a few hours; Windows is the slowest), then move
   kun's `RUSTY_V8_MIRROR_TAG` to the new tag.

Running the workflow by hand (`workflow_dispatch`) builds without
releasing; the files are kept as the run's artifacts.

## Local modifications

Code changes are marked with `kun:` comments.

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
  in `src/binding.cc`; experimental, kun's RFC 0001, Q18):
  `v8::cppgc::standalone::StandaloneHeap` is a `cppgc::Heap`, not a
  `v8::CppHeap`, for running with no isolate. It schedules its own GCs and
  posts them as non-nestable foreground tasks to a `PlatformImpl` (the
  callbacks `CustomPlatform` uses, with a null isolate), and never scans
  the stack (`kNoConservativeStackScan`). Objects go on any heap's
  `AllocationHandle` with `make_garbage_collected_on`
  (`cppgc__make_garbage_collectable_on`; upstream's
  `make_garbage_collected` shares its Rust code but still calls its own C
  function).
- **wasm GC objects and a weak table** (`src/cppgc/wasm_gc.rs`, C++ in
  `src/binding.cc`; only with pointer compression; experimental, kun's
  RFC 0001, Q15 and Q16): `WasmGcObject` is a C++ GC object with a type
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

Otherwise `kun` is upstream's `v152.2.0` plus this file.

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
2. `git submodule update --init --recursive --depth 1`.
3. Reapply and recheck the local modifications listed above: upstream's
   changes to `make_garbage_collected`, `RustObj`, and the C functions the
   slots call (`cppgc__Member__*`, `v8__TracedReference__*`).
4. In kun: rebuild from source (`python3 scripts/ci.py v8`, linked with
   `RUSTY_V8_ARCHIVE`), then `cargo test --workspace` and
   `python scripts/ci.py matrix`, and recheck the assumptions in
   `crates/dom/soundness.md` (its "需要人工審查的地方" covers a `v8`
   upgrade).
5. Release it (see "Releases", with `n` back to 1) and point kun at the new
   tag.
