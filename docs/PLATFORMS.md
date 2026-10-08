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

Three contracts hold this boundary together. Both surfaces share one
implementation of the first two (`caught` and `kintsugi_free` in
`crates/kintsugi-android/src/lib.rs`), so the C ABI and the JNI bridge cannot
drift apart in how they keep them, and the parts a test can reach have tests:

1. **Ownership.** Every returned string is allocated by Rust and released by
   `kintsugi_free`. Rust allocated it, Rust frees it, same allocator.
   (`tests/c_abi.c` calls `free(NULL)` to prove the no-op path.)
2. **No unwinding across the boundary.** Every entry point — the C ABI's
   `guard` and the JNI bridge's `answer` — runs its whole body, argument
   marshalling and `require` included, through the same `caught` helper, which
   turns a panic into an error *string*; unwinding into a JVM or a C caller is
   undefined behaviour. `caught` is unit-tested with a closure that panics on
   purpose. The JNI functions themselves have **no** runtime test — there is no
   JVM in CI — so their evidence is compilation plus the `nm` name check below,
   not execution; the C ABI's green check does not stand in for them.
3. **Errors are text, not NULL.** A failed repair is information the UI can
   show. `error: not found: '/nope'` beats a blank screen every time.

And one check that is about the *names*: CI greps the built library with `nm`
for all five `Java_com_kintsugi_engine_EngineBridge_*` symbols. A mismatch
between the Kotlin declarations and the Rust exports is the classic Android
`UnsatisfiedLinkError` — the app installs, launches, and dies on the first tap.

Both checks run locally in four commands. The harness is C, so it is compiled
against the library rather than run by `cargo test` (a `cc` build dependency to
avoid one documented command is not a trade worth making):

```sh
cargo build -p kintsugi-android                        # the C ABI
clang -Wall -Wextra -Werror -Icrates/kintsugi-android/include \
      crates/kintsugi-android/tests/c_abi.c -Ltarget/debug -lkintsugi_android -o /tmp/c_abi
DYLD_LIBRARY_PATH=target/debug /tmp/c_abi              # LD_LIBRARY_PATH on Linux
cargo build -p kintsugi-android --features jni-bridge  # then the name check
nm -gU target/debug/libkintsugi_android.dylib | grep Java_com_kintsugi_engine
```

(`nm -gU` is macOS; Linux wants `nm -D --defined-only`.)

## Exit codes (CLI)

Scriptable behaviour, so a repair can run inside make/CI:

| code | meaning |
| --- | --- |
| `0` | the repair was done, or help was asked for (`kintsugi`, `help`, `--help`, `-h`) |
| `1` | the engine refused (unreadable file, unmappable text, no such asset, a factor past what the input can bear) |
| `2` | the command line was wrong (unknown command, unknown flag, missing directory, a flag value that is not what it claims, or a request the tool refuses to carry out) |

Usage errors are separated from engine errors on purpose: `--faktur 4` should
not look like a broken game file. This was written down before it was true —
`--faktur 4` printed the usage text and exited **0**, which told a script its
repair had been made when nothing ran at all, and a missing directory exited
`1`, blaming the game for a command that was never finished. The table is now
tested through the binary, case by case, in `crates/kintsugi/tests/cli.rs`.

The line between the two is whether the tool could have known before opening
the game. `--factor 0` is wrong on its face (exit `2`); `--factor 400` on a
four-frame sequence is 1201 frames of real work, and only the sequence length
says whether that fits the frame budget, so its refusal is the engine's (exit
`1`) and names the input. The glazer's bound is the exception that proves the
rule: `1..=16` is fixed and known in advance, so `--factor 17` is exit `2`. One refusal sits in this class deliberately:
writing anything inside the game folder exits `2`, because overwriting an
original is a mistake in the *request*, not a fault in the game — and the files
there are verified byte-identical afterwards (`ensure_outside_game` in
`crates/kintsugi/src/main.rs`).

`install` is the one command that writes a game-shaped thing, and it writes it
where the user said: `--into COPY` is refused inside the game folder like every
other output, and the copy is mounted and read back afterwards so "the repair
landed" is a measurement. Its `--script FILE` is the patch (`translate
--write-script` output), never a path inside the game. The destination has three
allowed states and no fourth: empty, **verifiably Kintsugi's own previous
install** (every file it recorded in `.kintsugi-install` present with the
recorded size and checksum, and nothing else in the folder), or refused by name
— a save game, a hand-edited script, or a folder that was never ours.

The rule is about the folder rather than about one file, because "do not
overwrite the script you read" still allows `--write-script game/game.snn`, an
original that merely is not the script. So the game folder is read-only to this
tool: `--write-script`, `upscale -o`, and `interpolate -o` all refuse to land in
it, and the message suggests a path beside it. `demo` is the exception that
proves the rule — it *creates* its folder, leaves a `.kintsugi-demo` marker, and
refuses to write into a folder that has files but no marker, so
`demo --dir <a real game>` cannot plant fixtures over someone's installation.

What the check cannot see is a hard link to an original made **outside** the
folder (`ln game/story.bdt /tmp/other.bdt`): that is the same bytes under a
second name, and catching it needs file identity (device + inode, or volume +
file index), for which portable std has no call. The doc comment says that out
loud instead of implying a guarantee it cannot keep.

## What CI verifies, per platform

Every job builds its artifact, and every job whose artifact is a program
**runs it**; the APK cannot be run here, so that job unpacks it instead:

| job | checks |
| --- | --- |
| `verify` | `cargo fmt --check`, `cargo clippy -D warnings`, `cargo test --workspace`, the C ABI harness, the `nm` JNI symbol check, and — on a tag — that the tag matches the version inside the binary it just built |
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
