plugins {
    id("com.android.application")
}

// The ABIs to build. Phones and tablets are arm64-v8a. For the emulator add x86_64 (and
// `rustup target add x86_64-linux-android`):  gradlew assembleDebug -PrustAbis=arm64-v8a,x86_64
val rustAbis: List<String> =
    (providers.gradleProperty("rustAbis").orNull ?: "arm64-v8a")
        .split(",")
        .map { it.trim() }
        .filter { it.isNotEmpty() }

android {
    namespace = "com.indicatrixsuite.mobile"
    // The installed SDK platform (android-37.0). The target stays at 36, the level Play
    // requires: targeting 37 would opt into API 37 behaviour changes nobody has tested.
    compileSdk = 37

    defaultConfig {
        applicationId = "com.indicatrixsuite.mobile"
        minSdk = 28
        targetSdk = 36
        versionCode = 1
        versionName = "0.1.0"

        ndk {
            abiFilters += rustAbis
        }
    }

    buildTypes {
        release {
            // R8 keeps MainActivity whole (see proguard-rules.pro): Rust calls its methods
            // by name. Signing is the owner's job; this build type is unsigned.
            isMinifyEnabled = true
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
    }

    // Built-in Kotlin takes its JVM target from targetCompatibility.
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    packaging {
        jniLibs {
            // Store the .so uncompressed and page-aligned (16 KB devices, Play requirement).
            useLegacyPackaging = false
        }
    }
}

dependencies {
    // FileProvider (share sheet) and WindowCompat.
    implementation("androidx.core:core-ktx:1.15.0")
}

// ---------------------------------------------------------------------------------------
// Rust. `cargo ndk` builds libindicatrix_mobile_android.so for every ABI in `rustAbis`
// into src/main/jniLibs/<abi>/, and the JNI-libs merge tasks wait for it, so every
// assemble rebuilds (cargo itself is incremental) and the APK can never carry a stale
// library. The first build compiles Skia from source and takes a long time.
// ---------------------------------------------------------------------------------------

// android/ -> indicatrix-mobile-android/ -> apps/ -> the workspace root (the Cargo workspace).
val cargoWorkspace: File = rootProject.projectDir.resolve("../../..").canonicalFile
val jniLibsDir: File = projectDir.resolve("src/main/jniLibs")

// Slint's Android backend compiles a small Java helper in its build script and looks for
// `android.jar` itself; it does not find platform folders named like `android-37.0` (and
// cargo-ndk's own API level points it at a platform that is not installed). Hand it the
// compile platform's jar explicitly.
val sdkDirectory: Provider<Directory> = androidComponents.sdkComponents.sdkDirectory
val compileSdkLevel: Int = checkNotNull(android.compileSdk) { "compileSdk is not set" }

fun androidJarIn(sdk: File): File? =
    sdk.resolve("platforms").listFiles()
        ?.filter { it.isDirectory && it.name.startsWith("android-$compileSdkLevel") }
        ?.map { it.resolve("android.jar") }
        ?.firstOrNull { it.isFile }

fun registerCargoTask(taskName: String, release: Boolean): TaskProvider<Exec> =
    tasks.register<Exec>(taskName) {
        group = "rust"
        description =
            "Builds the Rust library with cargo-ndk (${if (release) "android-release" else "android-dev"} profile)."
        workingDir = cargoWorkspace
        // The SDK location is only resolved when the task runs.
        doFirst {
            val sdk = sdkDirectory.get().asFile
            environment("ANDROID_HOME", sdk.absolutePath)
            androidJarIn(sdk)?.let { environment("ANDROID_JAR", it.absolutePath) }
        }
        val command = mutableListOf("cargo", "ndk")
        rustAbis.forEach { abi -> command += listOf("-t", abi) }
        command += listOf("-o", jniLibsDir.absolutePath, "build", "-p", "indicatrix-mobile-android")
        command += listOf("--profile", if (release) "android-release" else "android-dev")
        commandLine(command)
    }

val cargoBuildDebug = registerCargoTask("cargoBuildDebug", release = false)
val cargoBuildRelease = registerCargoTask("cargoBuildRelease", release = true)

tasks.configureEach {
    when (name) {
        "mergeDebugJniLibFolders" -> dependsOn(cargoBuildDebug)
        "mergeReleaseJniLibFolders" -> dependsOn(cargoBuildRelease)
    }
}
