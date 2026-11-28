//! Required Notice: Copyright (c) 2026 Alazy Inc. (https://alazy.app)
//!
//! The relay client checks certificates with Android's own trust manager, which Rust can reach
//! only through the JVM. The app calls this once, before the first relay call, from Kotlin, on
//! the library that carries the relay client:
//!
//! ```kotlin
//! package app.alazy.link
//! object Tls { @JvmStatic external fun init(context: android.content.Context) }
//! ```
//!
//! and its Gradle build adds the `rustls-platform-verifier-android` artifact at the version in
//! Cargo.lock (see that crate's README for the repository entry).

use jni::objects::JObject;
use jni::{EnvUnowned, errors::ThrowRuntimeExAndDefault};

#[unsafe(no_mangle)]
pub extern "system" fn Java_app_alazy_link_Tls_init<'local>(
    mut env: EnvUnowned<'local>,
    _this: JObject<'local>,
    context: JObject<'local>,
) {
    env.with_env(|env| rustls_platform_verifier::android::init_with_env(env, context))
        .resolve::<ThrowRuntimeExAndDefault>()
}
