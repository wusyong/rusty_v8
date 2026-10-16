// kun: a Perfetto tracing session for the embedder, and its own track
// events, in the same trace as V8's (V8 is built with `v8_use_perfetto`).
// What Chromium's tracing does for Chrome, for a single process: V8 and the
// embedder write into Perfetto's in-process backend, one session records the
// categories asked for, and the trace streams into a file as it goes. See
// `src/kun_trace.rs`.

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

// The embedder's spans: one category, their names given at run time.
PERFETTO_DEFINE_CATEGORIES_IN_NAMESPACE(
    kun_trace,
    perfetto::Category("kun").SetDescription("The embedder's own spans"));
PERFETTO_TRACK_EVENT_STATIC_STORAGE_IN_NAMESPACE(kun_trace);

namespace {

// The one session a process records, between Start and Stop.
perfetto::TracingSession* session = nullptr;
int trace_fd = -1;

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

}  // extern "C"
