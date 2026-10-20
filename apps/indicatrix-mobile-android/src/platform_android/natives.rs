//! The `native*` methods Kotlin calls into Rust with.
//!
//! The JVM finds these by their mangled names (`Java_<package>_<class>_<method>`), so
//! they are plain exported `extern "system"` functions and need no registration. They
//! mirror the `external fun` declarations in `MainActivity.kt`; change both together.
//!
//! The protocol is: for a file pick, Kotlin calls [`nativeFileReady`] once per file and
//! then [`nativeRequestDone`] once; for a save it calls [`nativeSaveDone`] once. Request
//! id `0` is the incoming-files channel ("Open with", "Share to"), which uses the same
//! two pick calls without a request.

// The JNI export names are fixed by the JVM's name mangling and are not snake case, but
// rustc's `non_snake_case` skips `#[no_mangle]` functions with a foreign ABI, so no
// lint attribute is needed (an `expect` here would be unfulfilled).

use indicatrix_mobile_core::platform::PickedFile;
use jni::{
    Env, EnvUnowned,
    errors::{LogErrorAndDefault, Result as JniResult},
    objects::{JByteArray, JObject, JString},
    refs::Reference,
    sys::jlong,
};

use super::{jvm::read_byte_array, pending};

/// Reads a nullable Java string.
fn optional_string(env: &Env<'_>, text: &JString<'_>) -> JniResult<Option<String>> {
    if text.is_null() {
        Ok(None)
    } else {
        text.try_to_string(env).map(Some)
    }
}

/// `private external fun nativeFileReady(requestId: Long, name: String, bytes: ByteArray)`
///
/// One picked (or incoming) file, already read completely.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_indicatrixsuite_mobile_MainActivity_nativeFileReady<'caller>(
    mut unowned: EnvUnowned<'caller>,
    _this: JObject<'caller>,
    request_id: jlong,
    name: JString<'caller>,
    bytes: JByteArray<'caller>,
) {
    unowned
        .with_env(|env| -> JniResult<()> {
            let display_name = name.try_to_string(env)?;
            let bytes = read_byte_array(env, &bytes)?;
            pending::push_file(
                request_id as u64,
                PickedFile {
                    display_name,
                    bytes,
                },
            );
            Ok(())
        })
        .resolve::<LogErrorAndDefault>();
}

/// `private external fun nativeRequestDone(requestId: Long, error: String?)`
///
/// The end of a file pick: `error == null` completes it with the files delivered so far
/// (none means the user cancelled); otherwise it fails with that sentence.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_indicatrixsuite_mobile_MainActivity_nativeRequestDone<'caller>(
    mut unowned: EnvUnowned<'caller>,
    _this: JObject<'caller>,
    request_id: jlong,
    error: JString<'caller>,
) {
    unowned
        .with_env(|env| -> JniResult<()> {
            let error = optional_string(env, &error)?;
            pending::finish_documents(request_id as u64, error);
            Ok(())
        })
        .resolve::<LogErrorAndDefault>();
}

/// `private external fun nativeSaveDone(requestId: Long, displayName: String?, error: String?)`
///
/// The end of a save: `error` set is a failure, `displayName` set is a success, both
/// `null` is a cancel.
#[unsafe(no_mangle)]
pub extern "system" fn Java_com_indicatrixsuite_mobile_MainActivity_nativeSaveDone<'caller>(
    mut unowned: EnvUnowned<'caller>,
    _this: JObject<'caller>,
    request_id: jlong,
    display_name: JString<'caller>,
    error: JString<'caller>,
) {
    unowned
        .with_env(|env| -> JniResult<()> {
            let display_name = optional_string(env, &display_name)?;
            let error = optional_string(env, &error)?;
            pending::finish_save(request_id as u64, display_name, error);
            Ok(())
        })
        .resolve::<LogErrorAndDefault>();
}
