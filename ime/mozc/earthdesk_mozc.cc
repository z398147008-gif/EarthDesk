// EarthDesk: Mozc's conversion engine as a plain C library.
//
// The same thing Mozc's Android build does over JNI (android/jni/mozcjni.cc):
// one SessionHandler in-process, fed serialized commands::Command protobufs.
// No mozc_server.exe, no IPC, no Mozc TSF: 地球桌面输入法's engine process
// (EarthDeskIME.exe) loads this library and owns the keyboard side itself.
//
// Copied into Mozc's src/session/ and built with Bazel by
// ime/mozc/build.py (see .github/workflows/mozc.yml).
//
//   int  ed_mozc_init(const char* profile_dir_utf8)   0 = ok
//   int  ed_mozc_eval(const uint8_t* in, size_t n,     0 = ok; *out must be
//                     uint8_t** out, size_t* out_n)    freed with ed_mozc_free
//   void ed_mozc_free(uint8_t* p)
//   const char* ed_mozc_data_version()
//   void ed_mozc_shutdown()                            syncs the user data

#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <memory>
#include <mutex>
#include <string>

#include "base/system_util.h"
#include "engine/engine_factory.h"
#include "protocol/commands.pb.h"
#include "session/session_handler.h"

#ifdef _WIN32
#define ED_API extern "C" __declspec(dllexport)
#else
#define ED_API extern "C" __attribute__((visibility("default")))
#endif

namespace {

std::mutex g_mu;
std::unique_ptr<mozc::SessionHandler> g_handler;
std::string g_version;

}  // namespace

ED_API int ed_mozc_init(const char* profile_dir_utf8) {
  std::lock_guard<std::mutex> lock(g_mu);
  if (g_handler) return 0;
  if (profile_dir_utf8 != nullptr && profile_dir_utf8[0] != '\0') {
    mozc::SystemUtil::SetUserProfileDirectory(profile_dir_utf8);
  }
  auto engine = mozc::EngineFactory::Create();
  if (!engine.ok()) return 1;
  g_handler = std::make_unique<mozc::SessionHandler>(*std::move(engine));
  if (!g_handler->IsAvailable()) {
    g_handler.reset();
    return 2;
  }
  g_version = std::string(g_handler->GetDataVersion());
  return 0;
}

ED_API int ed_mozc_eval(const uint8_t* in, size_t in_len, uint8_t** out,
                        size_t* out_len) {
  if (out == nullptr || out_len == nullptr) return 3;
  *out = nullptr;
  *out_len = 0;
  mozc::commands::Command command;
  if (!command.ParseFromArray(in, static_cast<int>(in_len))) return 4;
  {
    std::lock_guard<std::mutex> lock(g_mu);
    if (!g_handler) return 5;
    g_handler->EvalCommand(&command);
  }
  const size_t n = command.ByteSizeLong();
  uint8_t* buf = static_cast<uint8_t*>(std::malloc(n > 0 ? n : 1));
  if (buf == nullptr) return 6;
  if (!command.SerializeToArray(buf, static_cast<int>(n))) {
    std::free(buf);
    return 7;
  }
  *out = buf;
  *out_len = n;
  return 0;
}

ED_API void ed_mozc_free(uint8_t* p) { std::free(p); }

ED_API const char* ed_mozc_data_version() { return g_version.c_str(); }

ED_API void ed_mozc_shutdown() {
  std::lock_guard<std::mutex> lock(g_mu);
  if (!g_handler) return;
  mozc::commands::Command sync;
  sync.mutable_input()->set_type(mozc::commands::Input::SYNC_DATA);
  g_handler->EvalCommand(&sync);
  g_handler.reset();
}
