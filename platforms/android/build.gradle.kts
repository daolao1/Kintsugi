// Root Gradle project: one module, and the plugins it needs.
//
// There is deliberately no Gradle wrapper checked in (it is a binary blob);
// CI installs Gradle 8.11.1 itself, and locally any Gradle 8.9+ works.
//
// Versions live here and nowhere else: the root declares the plugins with
// `apply false` so the classpath is built once, and `:app` applies them
// without repeating a version (which Gradle rejects).
plugins {
    id("com.android.application") version "8.7.3" apply false
    id("org.jetbrains.kotlin.android") version "2.0.21" apply false
}
