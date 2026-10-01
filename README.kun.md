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
- **The V8 sandbox**, and later patches of our own (for example to
  `src/binding.cc`; kun's `crates/dom/soundness.md` lists the upstream
  issues it works around).

Building from source alone doesn't need a fork (`V8_FROM_SOURCE=1` with the
crate's features); the fork is for when kun carries patches.

## Status

Evaluating building from source (kun's `docs/P3.md` decides what happens
next from the result). macOS, 2026-10-02, Apple M4: built with
`v8_enable_pointer_compression` in 23 minutes; kun's tests and
`scripts/ci.py matrix` pass against it, and `unwrap` checks the wrap tag
(kun's `bindings` test `unwrap_checks_the_wrap_tag`). Windows not tried
yet.

kun builds V8 once with `python3 scripts/ci.py v8` and keeps the result in
`../rusty_v8_artifacts`; see the README there.

## Local modifications

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
3. Reapply and recheck any local modifications listed above.
4. In kun: rebuild from source, then `cargo test --workspace` and
   `python scripts/ci.py matrix`, and recheck the assumptions in
   `crates/dom/soundness.md` (its "需要人工審查的地方" covers a `v8`
   upgrade).
