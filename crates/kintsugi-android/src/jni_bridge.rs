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

use jni::JNIEnv;
use jni::objects::{JClass, JString};
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
fn answer(env: &mut JNIEnv<'_>, body: impl FnOnce() -> crate::Result<String>) -> jstring {
    let text = match body() {
        Ok(text) => text,
        Err(e) => format!("error: {e}"),
    };
    give_string(env, text)
}

/// `EngineBridge.version(): String`
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kintsugi_engine_EngineBridge_version(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
) -> jstring {
    let text = format!("kintsugi {}", env!("CARGO_PKG_VERSION"));
    give_string(&mut env, text)
}

/// `EngineBridge.detect(dir: String): String`
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kintsugi_engine_EngineBridge_detect(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    dir: JString<'_>,
) -> jstring {
    let dir = take_string(&mut env, &dir);
    answer(&mut env, move || crate::detect(&require(dir)?))
}

/// `EngineBridge.play(dir: String, script: String?): String`
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kintsugi_engine_EngineBridge_play(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    dir: JString<'_>,
    script: JString<'_>,
) -> jstring {
    let dir = take_string(&mut env, &dir);
    let script = take_string(&mut env, &script);
    answer(&mut env, move || {
        crate::play(&require(dir)?, script.as_deref())
    })
}

/// `EngineBridge.demo(dir: String): String`
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kintsugi_engine_EngineBridge_demo(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    dir: JString<'_>,
) -> jstring {
    let dir = take_string(&mut env, &dir);
    answer(&mut env, move || crate::demo(&require(dir)?))
}

/// `EngineBridge.upscale(dir, asset, factor, method, outPath): String`
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_kintsugi_engine_EngineBridge_upscale(
    mut env: JNIEnv<'_>,
    _class: JClass<'_>,
    dir: JString<'_>,
    asset: JString<'_>,
    factor: jni::sys::jint,
    method: JString<'_>,
    out_path: JString<'_>,
) -> jstring {
    let dir = take_string(&mut env, &dir);
    let asset = take_string(&mut env, &asset);
    let method = take_string(&mut env, &method);
    let out_path = take_string(&mut env, &out_path);
    answer(&mut env, move || {
        crate::upscale_asset(
            &require(dir)?,
            &require(asset)?,
            factor.max(1) as u32,
            &require(method)?,
            &require(out_path)?,
        )
    })
}

/// Reject a missing argument with a message the UI can show.
fn require(value: Option<String>) -> crate::Result<String> {
    value.ok_or_else(|| kintsugi_core::error::Error::Plugin("missing argument".to_string()))
}
