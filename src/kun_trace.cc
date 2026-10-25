// kun: a Perfetto tracing session for the embedder, and its own track
// events, in the same trace as V8's (V8 is built with `v8_use_perfetto`).
// What Chromium's tracing does for Chrome, for a single process: V8 and the
// embedder write into Perfetto's in-process backend, one session records the
// categories asked for, and the trace streams into a file as it goes. See
// `src/kun_trace.rs`.

#include <atomic>
#include <cstddef>
#include <cstdint>
#include <cstdio>
#include <memory>

#if defined(_WIN32)
#include <fcntl.h>
#include <io.h>
#include <sys/stat.h>
#else
#include <fcntl.h>
#include <unistd.h>
#endif

#include "perfetto/base/logging.h"
#include "perfetto/tracing/core/trace_config.h"
#include "perfetto/tracing/tracing.h"
#include "perfetto/tracing/track_event.h"
#include "protos/perfetto/config/track_event/track_event_config.gen.h"

// The embedder's spans, their names given at run time, and the page's User
// Timing marks and measures under Blink's category for them.
PERFETTO_DEFINE_CATEGORIES_IN_NAMESPACE(
    kun_trace,
    perfetto::Category("kun").SetDescription("The embedder's own spans"),
    perfetto::Category("blink.user_timing")
        .SetDescription("The page's performance.mark() and measure()"));
PERFETTO_TRACK_EVENT_STATIC_STORAGE_IN_NAMESPACE(kun_trace);

namespace {

// The one session a process records, between Start and Stop.
perfetto::TracingSession* session = nullptr;
int trace_fd = -1;

// The memory the threads write their events into before the service copies
// them into the session's buffer: Perfetto's largest. A thread finding it
// full drops its events (the track event data source can't be made to
// wait), and the SDK's default of 256 KB fills under a GC's bursts of
// events (cppgc's marking steps on allocation, many a millisecond).
constexpr uint32_t kSharedMemoryKb = 32 * 1024;

int OpenForWriting(const char* path) {
#if defined(_WIN32)
  return _open(path, _O_WRONLY | _O_CREAT | _O_TRUNC | _O_BINARY,
               _S_IREAD | _S_IWRITE);
#else
  return open(path, O_WRONLY | O_CREAT | O_TRUNC, 0644);
#endif
}

// Perfetto's own messages: only its errors, not every session's start and
// end, on the embedder's stderr.
void LogErrors(perfetto::base::LogMessageCallbackArgs message) {
  if (message.level == perfetto::base::kLogError) {
    std::fprintf(stderr, "perfetto: %s:%d %s\n", message.filename,
                 message.line, message.message);
  }
}

void CloseFile(int fd) {
#if defined(_WIN32)
  _close(fd);
#else
  close(fd);
#endif
}

// How many measures have had a track.
std::atomic<uint64_t> measures = 0;

// A new measure's track id, spread over 64 bits (splitmix64 of a count, so
// never twice): the track's uuid is the id xored with its parent's, and the
// threads' tracks are uuids a thread id apart, so a small id would name
// another thread's track. Chrome's ids are hashes too.
uint64_t NextMeasureTrackId() {
  uint64_t x = (measures.fetch_add(1, std::memory_order_relaxed) + 1) *
               0x9e3779b97f4a7c15u;
  x = (x ^ (x >> 30)) * 0xbf58476d1ce4e5b9u;
  x = (x ^ (x >> 27)) * 0x94d049bb133111ebu;
  return x ^ (x >> 31);
}

// The trace's clock `ago_ns` before `now` (the trace's time): the
// embedder's own clock needn't be Perfetto's, but both have a now. Negative
// is in the future; nothing comes before the clock's zero.
uint64_t TraceTimeAgo(uint64_t now, int64_t ago_ns) {
  if (ago_ns < 0) return now + (uint64_t{0} - static_cast<uint64_t>(ago_ns));
  uint64_t ago = static_cast<uint64_t>(ago_ns);
  return ago < now ? now - ago : 0;
}

}  // namespace

extern "C" {

// Starts recording into the file at `path` (on Windows, in the C runtime's
// ANSI code page): the track events of `categories` (V8's and the
// embedder's `kun`), through a buffer of `buffer_kb` that is written out
// every second; if more than that comes in a second, the oldest of it is
// overwritten, which the trace's stats count as data loss (Perfetto wants a
// ring buffer with `write_into_file`: a `DISCARD` one loses data in ways
// that don't show). Before V8's platform is initialized, or V8 doesn't
// register its events. False if the file can't be written or a session
// was already started.
bool kun__Trace__Start(const char* path, const char* const* categories,
                       size_t category_count, uint32_t buffer_kb) {
  if (session != nullptr) return false;
  int fd = OpenForWriting(path);
  if (fd < 0) return false;

  perfetto::TracingInitArgs args;
  args.backends = perfetto::kInProcessBackend;
  args.log_message_callback = LogErrors;
  args.shmem_size_hint_kb = kSharedMemoryKb;
  perfetto::Tracing::Initialize(args);
  kun_trace::TrackEvent::Register();

  perfetto::TraceConfig config;
  auto* buffer = config.add_buffers();
  buffer->set_size_kb(buffer_kb);
  buffer->set_fill_policy(perfetto::TraceConfig::BufferConfig::RING_BUFFER);
  config.set_file_write_period_ms(1000);
  auto* source = config.add_data_sources()->mutable_config();
  source->set_name("track_event");
  perfetto::protos::gen::TrackEventConfig track_events;
  track_events.add_disabled_categories("*");
  for (size_t i = 0; i < category_count; i++) {
    track_events.add_enabled_categories(categories[i]);
  }
  source->set_track_event_config_raw(track_events.SerializeAsString());

  session =
      perfetto::Tracing::NewTrace(perfetto::kInProcessBackend).release();
  session->Setup(config, fd);
  session->StartBlocking();
  trace_fd = fd;
  return true;
}

// Stops recording: what is left in the buffer is written out and the file
// closed. Does nothing with no session.
void kun__Trace__Stop() {
  if (session == nullptr) return;
  kun_trace::TrackEvent::Flush();
  session->StopBlocking();
  delete session;
  session = nullptr;
  CloseFile(trace_fd);
  trace_fd = -1;
}

// Begins and ends a span of the embedder on this thread's track (nested
// spans end in the reverse order). `name` needn't be NUL-terminated or
// outlive the call. Off (one load and branch) unless `kun` is recorded.
void kun__Trace__Begin(const char* name, size_t name_len) {
  PERFETTO_USE_CATEGORIES_FROM_NAMESPACE_SCOPED(kun_trace);
  TRACE_EVENT_BEGIN("kun", perfetto::DynamicString(name, name_len));
}

void kun__Trace__End() {
  PERFETTO_USE_CATEGORIES_FROM_NAMESPACE_SCOPED(kun_trace);
  TRACE_EVENT_END("kun");
}

// A User Timing mark, as Blink's `UserTiming` writes one: an instant on this
// thread's track, at its time (`ago_ns` before now; a mark's time may be
// given, in the past or the future). `name` as for Begin. Off unless
// `blink.user_timing` is recorded.
void kun__Trace__Mark(const char* name, size_t name_len, int64_t ago_ns) {
  PERFETTO_USE_CATEGORIES_FROM_NAMESPACE_SCOPED(kun_trace);
  if (!TRACE_EVENT_CATEGORY_ENABLED("blink.user_timing")) return;
  TRACE_EVENT_INSTANT("blink.user_timing",
                      perfetto::DynamicString(name, name_len),
                      TraceTimeAgo(kun_trace::TrackEvent::GetTraceTimeNs(),
                                   ago_ns));
}

// A User Timing measure, as Blink writes one: a slice from its start to its
// end (each so long before now) on a track of its own under this thread's.
// Measures overlap as they please, even those of a name, which slices on
// one track can't. An end before the start is written as it is, as Chrome
// does; on its own track it spoils no other slice.
void kun__Trace__Measure(const char* name, size_t name_len,
                         int64_t start_ago_ns, int64_t end_ago_ns) {
  PERFETTO_USE_CATEGORIES_FROM_NAMESPACE_SCOPED(kun_trace);
  if (!TRACE_EVENT_CATEGORY_ENABLED("blink.user_timing")) return;
  // One now for both ends, or the end would be late by the begin's writing.
  uint64_t now = kun_trace::TrackEvent::GetTraceTimeNs();
  perfetto::Track track(NextMeasureTrackId(),
                        perfetto::ThreadTrack::Current());
  TRACE_EVENT_BEGIN("blink.user_timing",
                    perfetto::DynamicString(name, name_len), track,
                    TraceTimeAgo(now, start_ago_ns));
  TRACE_EVENT_END("blink.user_timing", track, TraceTimeAgo(now, end_ago_ns));
}

}  // extern "C"
