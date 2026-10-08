//! The JNI surface the Kotlin app in `platforms/android/` calls.
//!
//! This module is thin on purpose: every function converts JVM strings to
//! Rust strings, calls the same plain functions the C ABI calls, and converts
//! the answer back. If the two surfaces ever disagree, the bug is here and
//! nowhere else.
//!
//! Symbol names follow the JNI convention
//! `Java_<package>_<Class>_<method>`, with dots and underscores escaped. The
//! Kotlin side is `com.kintsugi.engine.EngineBridge`, so the names below are
//! `Java_com_kintsugi_engine_EngineBridge_*`.
//!
//! The second parameter is a [`JObject`], not a `JClass`, because the Kotlin
//! side is an `object`: its functions are *instance* methods on the singleton,
//! so the JVM passes the instance. (Adding `@JvmStatic` on the Kotlin side
//! would make them static and flip this to `JClass`; the two are the same
//! pointer at the ABI level, which is exactly why the mistake would go
//! unnoticed at run time and should be spelled out here.)
//!
//! # What is verified, and how
//!
//! Compilation and the exported symbol *names* are checked (CI greps the built
//! library with `nm`): a name mismatch is the classic Android
//! `UnsatisfiedLinkError` at first launch. The *execution* of these functions
//! is **not** covered by a test — there is no JVM in the test environment, and
//! `crates/kintsugi-android/tests/c_abi.c` exercises the C entry points, not
//! these. The no-unwinding rule below is shared with the C ABI ([`crate::caught`],
//! unit-tested with a panicking closure); the marshalling around it is verified
//! by reading it, not by running it.

use jni::JNIEnv;
use jni::objects::{JObject, JString};
use jni::sys::jstring;

/// Fetch a JVM string as a Rust `String`, treating null as absent.
fn take_string(env: &mut JNIEnv<'_>, value: &JString<'_>) -> Option<String> {
    if value.is_null() {
        return None;
    }
    env.get_string(value).ok().map(Into::into)
}

/// Hand a Rust `String` back to the JVM.
fn give_string(env: &mut JNIEnv<'_>, text: String) -> jstring {
    match env.new_string(text) {
        Ok(value) => value.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// Run a bridge call and turn both success and failure into JVM text.
///
/// The *whole* call runs inside [`crate::caught`] — argument marshalling and
/// `require` included, not just the engine call — so a panic in the bridge
/// itself cannot unwind into the JVM either. `caught` is the same helper the C
/// ABI's `guard` uses, so neither surface can drift from the other.
fn answer<'a>(
    env: &mut JNIEnv<'a>,
    body: impl FnOnce(&mut JNIEnv<'a>) -> crate::Result<String>,
) -> jstring {
    // The closure borrows `env` only for the length of the `caught` call; the
    // borrow ends before `give_string` needs `env` again.
    let text = crate::caught(|| body(&mut *env));
    give_string(env, text)
}

/// `EngineBridge.version(): String`
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kintsugi_engine_EngineBridge_version(
    mut env: JNIEnv<'_>,
    _this: JObject<'_>,
) -> jstring {
    answer(&mut env, |_env| {
        Ok(format!("kintsugi {}", env!("CARGO_PKG_VERSION")))
    })
}

/// `EngineBridge.detect(dir: String): String`
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kintsugi_engine_EngineBridge_detect(
    mut env: JNIEnv<'_>,
    _this: JObject<'_>,
    dir: JString<'_>,
) -> jstring {
    answer(&mut env, |env| {
        let dir = require(take_string(env, &dir))?;
        crate::detect(&dir)
    })
}

/// `EngineBridge.play(dir: String, script: String?): String`
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kintsugi_engine_EngineBridge_play(
    mut env: JNIEnv<'_>,
    _this: JObject<'_>,
    dir: JString<'_>,
    script: JString<'_>,
) -> jstring {
    answer(&mut env, |env| {
        let dir = require(take_string(env, &dir))?;
        let script = take_string(env, &script);
        crate::play(&dir, script.as_deref())
    })
}

/// `EngineBridge.demo(dir: String): String`
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kintsugi_engine_EngineBridge_demo(
    mut env: JNIEnv<'_>,
    _this: JObject<'_>,
    dir: JString<'_>,
) -> jstring {
    answer(&mut env, |env| {
        let dir = require(take_string(env, &dir))?;
        crate::demo(&dir)
    })
}

/// `EngineBridge.upscale(dir, asset, factor, method, outPath): String`
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kintsugi_engine_EngineBridge_upscale(
    mut env: JNIEnv<'_>,
    _this: JObject<'_>,
    dir: JString<'_>,
    asset: JString<'_>,
    factor: jni::sys::jint,
    method: JString<'_>,
    out_path: JString<'_>,
) -> jstring {
    answer(&mut env, |env| {
        let dir = require(take_string(env, &dir))?;
        let asset = require(take_string(env, &asset))?;
        let method = require(take_string(env, &method))?;
        let out_path = require(take_string(env, &out_path))?;
        crate::upscale_asset(&dir, &asset, factor.max(1) as u32, &method, &out_path)
    })
}

/// Reject a missing argument with a message the UI can show.
fn require(value: Option<String>) -> crate::Result<String> {
    value.ok_or_else(|| kintsugi_core::error::Error::Plugin("missing argument".to_string()))
}
