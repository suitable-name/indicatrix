# Pinned Android Back ownership patch

Source: crates.io `i-slint-backend-android-activity` **1.18.1**, upstream
<https://github.com/slint-ui/slint>. The package version, dependencies, copyright
headers and LICENSES are retained. Only this backend package is locally patched;
the renderer and other Slint crates remain upstream dependencies.

`application-owned-back.patch` contains the behavior change proposed for upstream:
an Activity metadata boolean, `dev.slint.applicationOwnsBack`, default false, skips
the backend's API 34+ dispatcher registration. Apps opting in own registration,
root navigation, IME handling and committed Back delivery. Indicatrix opts in and
provides that owner in BackNavigationController.

Legacy Back with a toolkit popup open dispatches Escape to dismiss the popup and
consumes that physical Back sequence, including repeats and release. Ordinary Back
retains upstream handling, including rejection at the app root. This preserves
dropdown-first navigation on API 28–32 as well as the app-owned API 33+ route.

Upstream 1.18.1 handles `MotionAction::Scroll` with `todo!()`, which panics on a mouse
wheel or trackpad scroll. The patched `androidwindowadapter.rs` returns
`InputStatus::Unhandled` for it (together with the hover enter and exit actions it already
ignored), so a scroll is dropped instead of ending the app. This change is not in
`application-owned-back.patch`; it is a second, independent local edit.

Two mechanical source moves keep each Rust file below 800 lines without changing
module visibility: androidwindowadapter.rs includes androidwindowadapter_input.rs
at the former `long_press_timeout` boundary; javahelper.rs includes
javahelper_callbacks.rs at the former `callback_update_text` boundary. Included
sources have the same lexical module/import scope. No Cargo registry file was edited.

A third local edit, in `java/SlintAndroidJavaHelper.java` (`onCreateInputConnection`): when the
focused Slint field is a password field (`InputType.password`, which the backend maps to
`TYPE_CLASS_TEXT | TYPE_TEXT_VARIATION_PASSWORD`), the editor info also carries
`IME_FLAG_NO_PERSONALIZED_LEARNING`, so a keyboard that honors it does not learn what is typed
(the one-time pairing token of the Remote page). Upstream sets only `IME_FLAG_NO_EXTRACT_UI`.
While a Material `LineEdit` shows its text through the reveal icon it is a plain text field
again and the flag is not set. Not in `application-owned-back.patch`. Not built or tested on a
device.

Remove the Cargo patch after an upstream release supplies the ownership switch and
the device navigation matrix passes. This patch has not been built or tested here.
