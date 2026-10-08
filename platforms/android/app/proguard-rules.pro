# Debug builds are not minified; release builds keep the JNI names intact.
# The engine's entry points are `external fun` on EngineBridge, and R8 must not
# rename or strip the class that owns the `Java_com_kintsugi_engine_...`
# symbols.
-keep class com.kintsugi.engine.EngineBridge { *; }
-keepclasseswithmembernames class * {
    native <methods>;
}
