//! kun: a Perfetto trace of the process, V8's events and the embedder's
//! spans in one file, as Chrome's trace has Blink's and V8's (V8 is built
//! with `v8_use_perfetto`; the C++ is `src/kun_trace.cc`).
//!
//! [`start`] before V8's platform is initialized (V8 registers its events
//! only once Perfetto is up), [`stop`] at the end; [`begin`] and [`end`]
//! from any thread in between. With no session nothing is recorded, and V8
//! doesn't even register its events.

use std::ffi::CString;
use std::ffi::c_char;
use std::path::Path;

unsafe extern "C" {
  fn kun__Trace__Start(
    path: *const c_char,
    categories: *const *const c_char,
    category_count: usize,
    buffer_kb: u32,
  ) -> bool;
  fn kun__Trace__Stop();
  fn kun__Trace__Begin(name: *const c_char, name_len: usize);
  fn kun__Trace__End();
}

/// Starts recording into the file at `path` the track events of
/// `categories` (V8's, such as `v8` and `devtools.timeline`, and `kun` for
/// [`begin`]'s spans), through a ring buffer of `buffer_kb` written out
/// every second; what overflows it within a second overwrites the oldest of
/// that second, counted as data loss in the trace's stats. False if the
/// file can't be written, `path` isn't UTF-8 (on Windows, the C runtime
/// takes it in the ANSI code page), or a session was already started. Call
/// it and [`stop`] from one thread.
pub fn start(path: &Path, categories: &[&str], buffer_kb: u32) -> bool {
  let Some(path) = path.to_str().and_then(|path| CString::new(path).ok())
  else {
    return false;
  };
  let categories: Vec<CString> = categories
    .iter()
    .filter_map(|category| CString::new(*category).ok())
    .collect();
  let pointers: Vec<*const c_char> = categories
    .iter()
    .map(|category| category.as_ptr())
    .collect();
  // SAFETY: the strings and the array outlive the call, which copies them.
  unsafe {
    kun__Trace__Start(
      path.as_ptr(),
      pointers.as_ptr(),
      pointers.len(),
      buffer_kb,
    )
  }
}

/// Stops recording: the rest of the trace is written out and the file
/// closed. Does nothing with no session.
pub fn stop() {
  // SAFETY: no arguments; the C++ checks for a session.
  unsafe { kun__Trace__Stop() }
}

/// Begins a span named `name` on this thread (category `kun`); [`end`] ends
/// the innermost one this thread began. A load and a branch when `kun`
/// isn't being recorded.
pub fn begin(name: &str) {
  // SAFETY: the C++ reads `name_len` bytes of `name` during the call (a
  // Perfetto `DynamicString`, copied into the trace), no NUL needed.
  unsafe { kun__Trace__Begin(name.as_ptr() as *const c_char, name.len()) }
}

/// Ends the innermost span [`begin`] began on this thread.
pub fn end() {
  // SAFETY: no arguments.
  unsafe { kun__Trace__End() }
}
