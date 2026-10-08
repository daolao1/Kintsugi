// The JVM's door into the engine.
//
// Symbol names are the JNI convention `Java_com_kintsugi_engine_EngineBridge_*`,
// which is exactly what `crates/kintsugi-android/src/jni_bridge.rs` exports —
// verified with `nm` on the built library, because a mismatch here is the
// classic Android `UnsatisfiedLinkError` at first launch.
package com.kintsugi.engine

object EngineBridge {
    init {
        // Loads `libkintsugi_android.so` from `jniLibs/<abi>/`.
        System.loadLibrary("kintsugi_android")
    }

    external fun version(): String

    /** Detection verdicts for a game directory, one per line. */
    external fun detect(dir: String): String

    /** Play one script (`script = null` picks the first one found). */
    external fun play(dir: String, script: String?): String

    /** Write the synthetic demo game into `dir` and run it end to end. */
    external fun demo(dir: String): String

    /** Glaze an image asset into a PNG file; returns a one-line report. */
    external fun upscale(
        dir: String,
        asset: String,
        factor: Int,
        method: String,
        outPath: String,
    ): String
}
