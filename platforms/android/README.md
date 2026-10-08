# 🏺 Kintsugi on Android

The APK is a **shell**: a Kotlin activity around `libkintsugi_android.so`. All
the engine work — detection, archive mounting, script interpretation, image
decoding, and Anime4K-style upscaling — happens in the same Rust crates the
desktop CLI uses, reached through the JNI bridge in
[`crates/kintsugi-android`](../../crates/kintsugi-android).

```
Kotlin (MainActivity)  →  EngineBridge (external fun)  →  libkintsugi_android.so
                                                          ├── kintsugi-core    (body)
                                                          ├── kintsugi-bluegale (seam)
                                                          └── kintsugi-video   (glaze)
```

## Building

Requires the Android SDK, the NDK (any r26+), `cargo-ndk`, and the four Rust
targets:

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi \
                  x86_64-linux-android i686-linux-android
cargo install cargo-ndk

# From the repository root: build the engine for every ABI the APK ships.
cargo ndk -t arm64-v8a -t armeabi-v7a -t x86_64 -t x86 \
    -o platforms/android/app/src/main/jniLibs \
    build --release -p kintsugi-android --features jni-bridge

# Then assemble the APK.
cd platforms/android && gradle assembleDebug
# → app/build/outputs/apk/debug/app-debug.apk
```

`cargo ndk` needs the NDK path in `ANDROID_NDK_HOME` (or `ANDROID_NDK_ROOT`).
CI does exactly the above — see [`.github/workflows/release.yml`](../../.github/workflows/release.yml) —
so the commands in this file are the ones that are actually exercised.

There is no Gradle wrapper in the tree on purpose: the wrapper is a binary
blob, and CI installs Gradle itself. Any Gradle 8.x works locally.

## Trying it

Install the APK and press **Run the demo game**. The app writes a tiny
synthetic BlueGale-style game into its private storage, detects it, plays the
Japanese kintsugi script, and glazes the title screen 4× — no game files
required, and no copyrighted bytes anywhere.

The **Glaze 4×** button shows the actual upscaled bitmap above the transcript,
so the claim and the pixels are both on screen.

## Using a real game (the honest limitation)

Android 10+ does not give apps raw filesystem paths to shared storage, and the
engine deliberately takes *paths*, not content URIs — it must be able to scan
a directory and mount archives itself. So putting a real game in means copying
it into the app's private directory first:

```sh
adb push /path/to/game /sdcard/game
adb shell run-as com.kintsugi.engine cp -r /sdcard/game files/game
```

A friendlier flow (a folder picker that copies through the Storage Access
Framework, or `MANAGE_EXTERNAL_STORAGE` for a read-only scan) is a shell-level
feature and is not implemented yet. It is listed in
[`docs/PLATFORMS.md`](../../docs/PLATFORMS.md) rather than quietly implied.

## What is not here yet

* **Signed release APKs** — release signing needs a keystore from CI secrets.
  Debug APKs are signed automatically and install fine.
* **A two-way choice protocol** — the host auto-answers choices with the first
  option and *says so* in the transcript. Letting the player tap a branch means
  sending a question back over the bridge; the engine side is already shaped
  for it (`Host::choose`).
* **Audio playback** — the seam reads audio containers, but nothing decodes
  Ogg Vorbis for `AudioTrack` yet.
