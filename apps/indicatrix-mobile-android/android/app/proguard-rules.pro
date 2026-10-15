# The Rust library calls MainActivity's methods by name over JNI and the JVM finds
# MainActivity's `external fun` natives by name. Keep the whole class and every native.
-keep class com.indicatrixsuite.mobile.MainActivity { *; }
-keepclasseswithmembernames class * {
    native <methods>;
}

# NativeActivity is instantiated by the framework from the manifest.
-keep class * extends android.app.NativeActivity
