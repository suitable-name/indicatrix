# Indicatrix for Android

The phone and tablet app: a touch-first [Slint](https://slint.dev) UI over
`indicatrix-mobile-core` (the shared, UI-free core), packaged as a Gradle project with a thin
Kotlin `NativeActivity` shim. The same UI also runs in a desktop window for development
(the desktop preview, below).

**Status.** Version 0.9.0 (versionCode 3). There is an internal testing track on Google Play,
but no public Google Play listing yet. Anyone can build the app and install it on their own
Android phone or tablet: a debug APK from Android Studio or from `gradlew assembleDebug` plus
`adb install` (see "Build and run"). The same UI also runs in a desktop window with the
preview, no device needed.

## Screenshots

Phone (390 x 800):

<table>
<tr>
<td><img src="../../assets/mobile/phone_library.png" width="220" alt="Library"></td>
<td><img src="../../assets/mobile/phone_design_solid.png" width="220" alt="Design, Solid view"></td>
<td><img src="../../assets/mobile/phone_design_diagram.png" width="220" alt="Design, Diagram view"></td>
<td><img src="../../assets/mobile/phone_design_render.png" width="220" alt="Design, Render view"></td>
<td><img src="../../assets/mobile/phone_tiers_sheet.png" width="220" alt="Design, Tiers sheet"></td>
</tr>
<tr>
<td><img src="../../assets/mobile/phone_stone.png" width="220" alt="Design, Stone tab"></td>
<td><img src="../../assets/mobile/phone_light.png" width="220" alt="Design, Light tab"></td>
<td><img src="../../assets/mobile/phone_checks.png" width="220" alt="Design, Checks tab"></td>
<td><img src="../../assets/mobile/phone_cut.png" width="220" alt="Cut"></td>
<td><img src="../../assets/mobile/phone_more.png" width="220" alt="More"></td>
</tr>
</table>

Tablet, landscape (1280 x 800):

<img src="../../assets/mobile/tablet_library.png" width="640" alt="Library on a tablet">
<img src="../../assets/mobile/tablet_design_render.png" width="640" alt="Design, Render view on a tablet">
<img src="../../assets/mobile/tablet_cut.png" width="640" alt="Cut on a tablet">

The pictures come from the desktop preview: `cargo ui-screens-mobile` (from the repository
root) rewrites `assets/mobile/`.

```
apps/indicatrix-mobile-android/
  src/android_entry.rs        android_main: logging, Slint backend, platform, run()
  src/platform_android/       the Platform implementation over JNI (Rust side)
  src/run.rs, src/design/...  the run loop and the Rust side of each screen
  ui/                         the Slint UI
  android/                    the Gradle project (Kotlin side, manifest, cargo-ndk task)
```

Package and application id: `com.indicatrixsuite.mobile`. versionName 0.9.0, versionCode 3,
minSdk 28, targetSdk 37, compileSdk 37, ABI `arm64-v8a`.

## One-time setup (Windows)

1. **Android Studio** (current stable) with, from the SDK Manager: Android SDK Platform 37
   (the `compileSdk`; the app targets API 36),
   Build-Tools (current), and **NDK r30** (side by side). The Gradle daemon asks for a
   JetBrains JDK 21 (`gradle/gradle-daemon-jvm.properties`; the foojay resolver downloads it
   when none is installed, and the JDK bundled with Android Studio, `<Android Studio>\jbr`,
   also fits). The Java and Kotlin sources compile for Java 17.
2. **Rust target and cargo-ndk:**
   ```
   rustup target add aarch64-linux-android
   cargo install cargo-ndk --version 4.1.2 --locked
   ```
   For an emulator also `rustup target add x86_64-linux-android`.
3. **Environment variables** (user level, then restart Android Studio and shells):
   - `ANDROID_HOME` = the SDK directory, e.g. `C:\Users\<you>\AppData\Local\Android\Sdk`
   - `ANDROID_NDK_HOME` = `%ANDROID_HOME%\ndk\<r30 version folder>`
   - `JAVA_HOME` = a JDK 17 or newer directory (the Slint Android backend's build script runs
     `javac` and D8 against the platform's `android.jar`)
   - `ANDROID_JAR` = `%ANDROID_HOME%\platforms\android-37.0\android.jar`, needed for
     cargo commands you run yourself: the Slint build script cannot find a platform folder
     named `android-37.0` on its own and panics with "No Android platforms found". The
     Gradle cargo tasks set it automatically.
4. Skia is compiled from source on the first Rust build and needs host `clang`, Python and
   `ninja` on `PATH`. The first build is slow (tens of minutes); later builds are
   incremental. If the Windows-host cross build fails, build in WSL2 or on the Linux worker.
5. **Gradle wrapper.** The repository carries `gradle/wrapper/gradle-wrapper.properties`
   (Gradle 9.6.0) but **not** `gradle-wrapper.jar` or the `gradlew` scripts. Open
   `android/` in Android Studio once and let it sync: it creates them. (Alternatively run
   `gradle wrapper` in `android/` with any installed Gradle.)

Versions: Gradle 9.6.0, AGP 9.4.1 with its built-in Kotlin (no Kotlin plugin), and
`androidx.core` 1.15.0. `gradle.properties` carries no compatibility flags, so the build
uses AGP 9's defaults and the new variant API; keep it that way when Android Studio offers
an upgrade (its upgrade assistant adds `android.newDsl=false` and similar flags, which
only bring the deprecation warnings back).

## Build and run

**Android Studio:** open `apps/indicatrix-mobile-android/android`, pick a device
(USB debugging on) and press Run. The Gradle task `cargoBuildDebug` runs
`cargo ndk -t arm64-v8a -o app/src/main/jniLibs build -p indicatrix-mobile-android
--profile android-dev` from the repository root before every assemble, so the APK always carries a fresh library.

**Command line**, from `apps/indicatrix-mobile-android/android`:
```
gradlew assembleDebug
adb install -r app\build\outputs\apk\debug\app-debug.apk
adb shell am start -n com.indicatrixsuite.mobile/.MainActivity
```
For the emulator add `-PrustAbis=arm64-v8a,x86_64`. A release build is
`gradlew assembleRelease` (or `bundleRelease`), which builds the Rust library with
`--profile android-release`; the result is unsigned, signing and Play upload are the
owner's job.

**Rust only** (to see compiler errors without Gradle), from the repository root:
```
cargo ndk -t arm64-v8a build -p indicatrix-mobile-android --profile android-dev
```
The output is `target\aarch64-linux-android\android-dev\libindicatrix_mobile_android.so`.

**Page size.** Play requires 16 KB page support for updates from 2027-02-01. NDK r28+
links 16 KB aligned by default. Verify a built library with
`llvm-objdump -p libindicatrix_mobile_android.so` (every `LOAD` line should say
`align 2**14`) and an APK with `zipalign -c -P 16 4 app-debug.apk`.

## Logs

```
adb logcat -s indicatrix slint
```
`indicatrix` carries the app's own `log` output, the `tracing` events of the shared
crates (default level `info`) and Rust panics with a backtrace (addresses only in release
builds, which are stripped). `slint` is Slint's own tag.

## Desktop preview

The same UI in a desktop window at a phone or tablet size, with file pickers from `rfd`:
```
cargo run -p indicatrix-mobile-android --features desktop-preview --bin indicatrix-mobile-preview -- --size 390x800
```
(from the repository root). It exercises the UI and the core, not the Android services.

## How the Android services work

The Kotlin `MainActivity` is the NativeActivity instance, so Rust calls its public methods
by name over JNI (`src/platform_android/mod.rs`): `openDocuments`, `createDocument`,
`share`, `printHtml`, `setClipboardText`, `keepScreenOn`, `thermalHeadroom`, `openUrl`,
`cacheDirPath`, `systemPowerSave`, `networkMetered` (1 metered, 0 unmetered, -1 unknown),
`setBackTarget`, and `storeRemoteCredentials` / `loadRemoteCredentials` /
`forgetRemoteCredentials` (the Keystore-wrapped remote credentials; Forget Server deletes the
encrypted file and the Keystore key `indicatrix.remote.credentials.v1`). Most return `null` or a
message for the user; `loadRemoteCredentials` returns the bundle, or lets a Java exception
escape, which Rust reports as "enroll again".
Answers to asynchronous requests (pick, save) come back through three `native*` functions
(`src/platform_android/natives.rs`) tagged with the request id, and Back commits through two
more (`src/platform_android/back.rs`); files that other apps send
("Open with", "Share to") use request id 0 and are held until the UI registers its
handler. Limits on files read from other apps: 64 MiB per file, 256 MiB per request, 2,000
design files per picked folder.

## Privacy controls

The app asks for two normal permissions only: `INTERNET` and `ACCESS_NETWORK_STATE`.
Automatic backup is off (`allowBackup="false"`). These controls remove what the app stores:

- **Forget Server** (Remote page) asks first, then deletes the stored pairing credentials
  with the Android Keystore key that protects them, and clears the saved server name and
  address. Pairing again needs a new one-time token.
- **Clear Recent Items** (More page) empties the list of recently opened designs (at most
  10 are kept); the designs themselves are not deleted.
- **Delete All Cutting Progress** (Preferences, and the Cut menu) asks first, then deletes the
  done marks of every design on the device, not only those of the open design.
- **WebView metrics opt-out.** Printing creates a system WebView. The manifest sets
  `android.webkit.WebView.MetricsOptOut`, so the WebView does not upload usage and crash
  metrics to Google, even when device-level usage and diagnostics are on.

## Known limitations

- **Ray-Traced and Path-Traced.** The Render page offers both engines. Ray-Traced
  finishes after fixed AA taps: Draft/Normal/Fine use 1/4/8 taps, and export uses
  16. Fire traces three fixed indices; this is an approximation, with no
  birefringent doubling, polarization, scattering, zoning or edge
  rounding. Ray-Traced has
  no denoising. Its device GPU route supports every material class with a
  1,048,576-pixel limit and at least four storage bindings; Path-Traced retains
  its 262,144-pixel GPU limit. Unsupported device workloads use the processor.
  Remote Ray-Traced requires matching protocol-v29 peers. Device image quality,
  driver behavior, thermal behavior and timing still require owner validation;
  no measured speedup is claimed here.

- **No TalkBack.** Slint exposes no accessibility tree on Android.
- **Feature expansion validation.** History, saved variants, concave editing, Deep
  Solve, Retarget, Optimize and device GPU routing have source implementations awaiting
  owner-run validation. Release builds include Optimize and Retarget through the
  default feature `validated-design-search`. See the
  [implementation status](../../docs/android-feature-expansion-implementation-status.md)
  for scope, review corrections and validation commands.
- **Remote rendering.** Authenticated sessions support coordinator DisplayOnly and
  explicit FullData, with analytic lighting or the device's effective HDR map (sent once
  as an exact versioned float asset to endpoints that advertise HDR). PNG export works in
  both modes (a coordinator delivers the finished picture for DisplayOnly) and is limited
  to 1,048,576 pixels. A live session that is interrupted reconnects by itself and restarts
  from sample zero; if the process is killed it does not survive. **Send To Server** (export
  sheet, Remote backend) stores a durable job on the endpoint instead; it keeps rendering
  after the phone disconnects, and Remote > Server Jobs lists, saves, cancels and deletes
  it. On a metered network, sessions that move many megabytes are refused unless Allow
  Metered Networks is on (the permission `ACCESS_NETWORK_STATE` reads the network
  state). Server jobs refuse color-zoned materials, and a worker renders them on its CPU
  lane. Automatic rendering stays on-device. This code has been written and read, not
  compiled or run; see the implementation status for the validation list.
- **External mouse wheel.** The Android adapter of Slint 1.18.1 has a `todo!()` for scroll
  events (mouse wheel, trackpad scroll). The vendored copy of
  `i-slint-backend-android-activity` (`[patch.crates-io]`) reports such an event as
  unhandled instead of panicking, so the wheel does nothing in the app; scrolling by touch
  is unaffected. Forwarding the wheel to Slint is not implemented.
- **Back navigation validation.** API 33+ uses the app-owned dispatcher paired with
  the pinned Slint backend ownership patch; API 28–32 retains the legacy key route.
  Root navigation belongs to Android, and cancelled predictive gestures do not commit
  navigation. Device validation remains required on API 28/32, 33, 34/35 and 36,
  Pixel and Samsung, gesture/three-button navigation, IME, rotation and rapid actions.
  See `vendor/slint-android-activity-1.18.1/PATCH.md`; do not independently change the
  manifest ownership metadata or dispatcher opt-in.

- **Soft keyboard.** Slint drives the keyboard through its own Java helper; numeric entry
  uses the app's own keypad, not the system numeric keyboard.
- **Printing** renders the cutting sheet in an off-screen `WebView`; the system print
  dialog's preview is the check that it came out right.
