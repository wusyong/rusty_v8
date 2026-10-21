// kun: V8's `CpuProfiler`, started and stopped by the embedder, its
// profile serialized by V8 as DevTools' JSON (`.cpuprofile`, what the
// `Profiler` domain's `stop` gives). The C++ is in `src/binding.cc`.

//! [`CpuProfiler`]: samples the isolate's stack (on a thread of its own,
//! every [`set_sampling_interval`](CpuProfiler::set_sampling_interval))
//! from [`start`](CpuProfiler::start) to [`stop`](CpuProfiler::stop), which
//! gives the profile as DevTools' JSON.

use std::ffi::c_void;
use std::ptr::NonNull;
use std::ptr::addr_of_mut;

use crate::isolate::Isolate;
use crate::isolate::RealIsolate;
use crate::support::Opaque;

#[repr(C)]
struct RealCpuProfiler(Opaque);

unsafe extern "C" {
  fn v8__CpuProfiler__New(isolate: *mut RealIsolate) -> *mut RealCpuProfiler;
  fn v8__CpuProfiler__Dispose(this: *mut RealCpuProfiler);
  fn v8__CpuProfiler__SetSamplingInterval(this: *mut RealCpuProfiler, us: i32);
  fn v8__CpuProfiler__StartProfiling(
    this: *mut RealCpuProfiler,
    isolate: *mut RealIsolate,
  ) -> bool;
  fn v8__CpuProfiler__StopProfiling(
    this: *mut RealCpuProfiler,
    isolate: *mut RealIsolate,
    callback: unsafe extern "C" fn(*mut c_void, *const u8, usize) -> bool,
    arg: *mut c_void,
  ) -> bool;
}

/// A V8 `CpuProfiler` of one isolate, disposed when dropped. It must be
/// dropped before its isolate (an isolate's slots are dropped after V8's
/// teardown, so it can't live in one), and used on the isolate's thread
/// while it is entered.
pub struct CpuProfiler {
  profiler: NonNull<RealCpuProfiler>,
  isolate: *mut RealIsolate,
  /// A profile is running: it is stopped before the profiler is disposed
  /// (disposing a profiler that is sampling leaves its sampling thread
  /// running).
  running: bool,
}

impl CpuProfiler {
  pub fn new(isolate: &mut Isolate) -> Self {
    let isolate = isolate.as_real_ptr();
    // SAFETY: `isolate` is a live isolate.
    let profiler = unsafe { v8__CpuProfiler__New(isolate) };
    Self {
      profiler: NonNull::new(profiler).expect("CpuProfiler::New"),
      isolate,
      running: false,
    }
  }

  /// The interval between samples, in microseconds, for the profiles
  /// started after it.
  pub fn set_sampling_interval(&mut self, us: i32) {
    // SAFETY: the profiler is live until dropped.
    unsafe { v8__CpuProfiler__SetSamplingInterval(self.profiler.as_ptr(), us) }
  }

  /// Starts a profile that records each sample; `false` if one is already
  /// running.
  pub fn start(&mut self) -> bool {
    // SAFETY: the profiler and its isolate are live, the isolate entered on
    // this thread (the C++ opens a handle scope for the title).
    let started = unsafe {
      v8__CpuProfiler__StartProfiling(self.profiler.as_ptr(), self.isolate)
    };
    self.running |= started;
    started
  }

  /// Stops the profile [`start`](Self::start) started and gives it as
  /// DevTools' JSON; `None` if none was running.
  pub fn stop(&mut self) -> Option<String> {
    unsafe extern "C" fn append(
      arg: *mut c_void,
      data: *const u8,
      size: usize,
    ) -> bool {
      // SAFETY: `arg` is the `Vec` below, `data` a chunk of `size` bytes V8
      // owns for the call.
      unsafe {
        let json = &mut *(arg as *mut Vec<u8>);
        if size > 0 {
          json.extend_from_slice(std::slice::from_raw_parts(data, size));
        }
      }
      true
    }
    self.running = false;
    let mut json = Vec::new();
    // SAFETY: as in `start`; `json` outlives the call.
    let stopped = unsafe {
      v8__CpuProfiler__StopProfiling(
        self.profiler.as_ptr(),
        self.isolate,
        append,
        addr_of_mut!(json) as *mut c_void,
      )
    };
    // V8 writes ASCII (JSON with `\u` escapes).
    stopped.then(|| String::from_utf8(json).expect("V8 writes ASCII JSON"))
  }
}

impl Drop for CpuProfiler {
  fn drop(&mut self) {
    if self.running {
      self.stop();
    }
    // SAFETY: disposed once, before its isolate (see the type's docs).
    unsafe { v8__CpuProfiler__Dispose(self.profiler.as_ptr()) }
  }
}
