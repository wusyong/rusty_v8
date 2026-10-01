# rusty_v8 (kun's fork)

| | |
| --- | --- |
| Upstream | https://github.com/denoland/rusty_v8 (remote `upstream`; fetch only, its push URL is disabled) |
| Fork | https://github.com/wusyong/rusty_v8 (remote `origin`) |
| Branch | `kun`, from upstream's tag `v152.2.0` (`2768994`), the `v8` crate version kun uses |
| License | MIT (`LICENSE`); V8 and the other submodules under their own licenses |
| Used by | kun's `bindings` and `dom`, by path, with `v8_enable_pointer_compression`; kun links the library built from it (`../rusty_v8_artifacts`) |

## Why a fork

kun wants to build V8 from source instead of using the prebuilt static
library (`docs/P3.md` in kun, "Wrapper 的型別資訊"):

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

kun builds V8 from this fork (kun's `docs/P3.md`, decision 10). macOS,
2026-10-02, Apple M4: built with `v8_enable_pointer_compression` in 23
minutes; kun's tests and `scripts/ci.py matrix` pass against it, and
`unwrap` checks the wrap tag (kun's `bindings` test
`unwrap_checks_the_wrap_tag`). kun keeps a debug and a release build (see
`../rusty_v8_artifacts/README.kun.md`). Windows not tried yet.

kun builds V8 once with `python3 scripts/ci.py v8` and keeps the result in
`../rusty_v8_artifacts`; see the README there.

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
- **Archive artifacts follow cargo's profile** (`build.rs`,
  `prebuilt_profile`): with `RUSTY_V8_ARCHIVE` set, a dev build links the
  `_debug_` library and a `--release` build the `_release_` one, as a build
  from source picks gn's `is_debug`. Upstream links `_release_` unless
  `V8_FORCE_DEBUG` is set. kun keeps both builds there (see
  `../rusty_v8_artifacts/README.kun.md`).
- `.gitignore`: `/gen/*.rs`. The build script copies the generated
  binding into `gen/` (from `RUSTY_V8_ARCHIVE`'s directory, or a download);
  upstream only tracks `gen/.gitkeep`, so it showed up as untracked.

Otherwise `kun` is upstream's `v152.2.0` plus this file.

## Layout and setup

This directory sits next to kun, at `../third_party/rusty_v8` from kun's
root. The submodules (V8, Chromium's `build`, `buildtools`, clang and
libc++, ICU...) were fetched with:

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
4. In kun: rebuild from source, then `cargo test --workspace` and
   `python scripts/ci.py matrix`, and recheck the assumptions in
   `crates/dom/soundness.md` (its "需要人工審查的地方" covers a `v8`
   upgrade).
