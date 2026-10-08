# Platforms: Android, macOS, Windows — maintained together

> 我需要同时维护 apk mac 和 win 的
> *"I need to maintain the APK, macOS, and Windows at the same time."*

The architecture's answer to that requirement is that **there is nothing to
maintain three times**. The body, the seams, and the glaze are one Rust
codebase; the platforms differ in exactly two places: how a window shows text,
and how a binary is packaged.

```
                     ┌───────────────────────────────┐
                     │  kintsugi-core + seams + glaze │   one codebase, no #[cfg]
                     └───────────────┬───────────────┘
                                     │
        ┌────────────────────────────┼────────────────────────────┐
        │                            │                            │
  kintsugi (CLI)            kintsugi-android (cdylib)      future shells
  TerminalHost              TranscriptHost + JNI           (web, iOS…)
        │                            │
  macOS · Windows · Linux     Android APK (4 ABIs)
```

## What each platform is made of

| platform | shell | host impl | engine delivered as | built by |
| --- | --- | --- | --- | --- |
| macOS | `crates/kintsugi` | `TerminalHost` | universal binary `kintsugi` (arm64 + x86_64) | `macos-latest`, `lipo` |
| Windows | `crates/kintsugi` | `TerminalHost` | `kintsugi.exe` (x86_64-msvc) | `windows-latest` |
| Linux | `crates/kintsugi` | `TerminalHost` | `kintsugi` (x86_64) | `ubuntu-latest` |
| Android | `platforms/android` | `TranscriptHost` | `libkintsugi_android.so` per ABI, inside the APK | `ubuntu-latest` + NDK + `cargo-ndk` + Gradle |

There is **no `#[cfg(target_os)]` anywhere in the engine crates**. If a change
requires one, the abstraction is wrong: it belongs behind `Host` or behind the
C ABI, not scattered through the body.

## The Android bridge, in one page

Two surfaces are exported from `crates/kintsugi-android`, over the same Rust
functions:

```c
// Plain C ABI — any language, any platform, tested from C in CI.
char *kintsugi_version(void);
char *kintsugi_detect(const char *dir);
char *kintsugi_play(const char *dir, const char *script);
char *kintsugi_demo(const char *dir);
char *kintsugi_upscale(const char *dir, const char *asset,
                       unsigned factor, const char *method, const char *out_path);
void  kintsugi_free(char *text);
```

```kotlin
// JNI — what the Kotlin app calls. Feature-gated so the crate still builds
// (and unit-tests) where there is no JVM.
object EngineBridge {
    external fun detect(dir: String): String
    external fun play(dir: String, script: String?): String
    external fun demo(dir: String): String
    external fun upscale(dir: String, asset: String, factor: Int,
                         method: String, outPath: String): String
}
```

Three contracts hold this boundary together, and each one has a test:

1. **Ownership.** Every returned string is allocated by Rust and released by
   `kintsugi_free`. Rust allocated it, Rust frees it, same allocator.
   (`tests/c_abi.c` calls `free(NULL)` to prove the no-op path.)
2. **No unwinding across the boundary.** Every entry point wraps its body in
   `catch_unwind` and turns a panic into an error *string*; unwinding into a
   JVM or a C caller is undefined behaviour.
3. **Errors are text, not NULL.** A failed repair is information the UI can
   show. `error: not found: '/nope'` beats a blank screen every time.

And one check that is about the *names*: CI greps the built library with `nm`
for all five `Java_com_kintsugi_engine_EngineBridge_*` symbols. A mismatch
between the Kotlin declarations and the Rust exports is the classic Android
`UnsatisfiedLinkError` — the app installs, launches, and dies on the first tap.

## Exit codes (CLI)

Scriptable behaviour, so a repair can run inside make/CI:

| code | meaning |
| --- | --- |
| `0` | the repair was done |
| `1` | the engine refused (unreadable file, unmappable text, no such asset) |
| `2` | the command line was wrong (unknown flag, missing value, or a request the tool refuses to carry out) |

Usage errors are separated from engine errors on purpose: `--faktur 4` should
not look like a broken game file. One refusal sits in this class deliberately:
pointing `--write-script` at the very file being read exits `2`, because
overwriting an original is a mistake in the *request*, not a fault in the game —
and the original is verified byte-identical afterwards (`same_file` in
`crates/kintsugi/src/main.rs`).

## What CI verifies, per platform

Every job builds and then **runs the artifact**:

| job | checks |
| --- | --- |
| `verify` | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace`, the C ABI harness, and the `nm` JNI symbol check |
| `macos` | both architectures build, `lipo` joins them, the universal binary's `demo` run produces the expected output and the glazed PNG exists |
| `windows` | same smoke test on `kintsugi.exe` |
| `linux` | same smoke test |
| `android` | `cargo-ndk` builds 4 ABIs, Gradle assembles the debug APK, and the job **unzips the APK** to prove all four `libkintsugi_android.so` slices are inside |

A platform that stops working turns a job red. That is the mechanism that keeps
"maintained together" true rather than aspirational.

## What is verified where (honest table)

| thing | verified locally on macOS | verified in CI |
| --- | --- | --- |
| body + seams + glaze tests | ✅ `cargo test --workspace` | ✅ |
| C ABI end to end (12 checks) | ✅ clang + `libkintsugi_android.dylib` | ✅ |
| JNI symbols exported with the right names | ✅ `nm` | ✅ |
| `jni-bridge` feature compiles | ✅ | ✅ |
| macOS release binary + smoke test | ✅ `cargo build --release` | ✅ |
| Windows binary + smoke test | ❌ no Windows toolchain here | ✅ `windows-latest` |
| Android NDK cross-build (4 ABIs) | ❌ no NDK/SDK here | ✅ |
| APK assembly + 4 ABI slices | ❌ | ✅ |
| Install and run on a real device/emulator | ❌ | ❌ *not automated yet* |

The last row is the one to be careful about: CI proves the APK is *well-formed
and complete*, not that a device renders it. Android UI testing (an
instrumented test or a screenshot check on an emulator) is the next honest step
and is listed under "Roadmap" below rather than implied by a green build.

## Roadmap for the shells

Ordered by what a user would notice first:

1. **Signed release APKs.** Release signing needs a keystore from CI secrets;
   debug APKs are signed automatically and install fine. (`.github/workflows/release.yml`
   already attaches the debug APK to tagged releases.)
2. **A folder picker in the Android app.** Android 10+ does not hand out raw
   filesystem paths, and the engine deliberately takes paths, not content URIs,
   because it must scan a directory and mount archives itself. The fix is a
   Storage Access Framework picker that copies the chosen tree into app-private
   storage, or `MANAGE_EXTERNAL_STORAGE` for a read-only scan. Until then:
   `adb push` + `run-as` (see `platforms/android/README.md`).
3. **Interactive choices on Android.** Today `TranscriptHost` answers the first
   option and says so in the transcript. Two-way choices need a question to
   travel back over the bridge; the engine side is already shaped for it
   (`Host::choose`).
4. **Audio playback.** The seams read audio containers; nothing decodes them
   for `AudioTrack` yet.
5. **An instrumented Android test**, so "the APK works" stops being an
   inference from "the APK is complete".
6. **iOS**, which is the same `cdylib` behind a `TranscriptHost`-style shell —
   the C ABI exists precisely so this is a week of work, not a rewrite.

## Adding a platform

If you are porting Kintsugi to a new place, you need three things and nothing
else:

1. a way to call Rust from that platform (the C ABI is already there);
2. a `Host` implementation that shows text and answers choices;
3. a packaging step that ships the compiled library.

Do **not** port the engine. If a platform needs engine-level changes, that is a
bug in the layering — bring it to `kintsugi-core` behind a trait, where every
seam gets it for free.
