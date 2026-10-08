// Root Gradle project: one module, and the plugins it needs.
//
// There is deliberately no Gradle wrapper checked in (it is a binary blob);
// CI installs Gradle itself, and locally any Gradle 8.x works.
plugins {
    id("com.android.application") version "8.7.3" apply false
    id("org.jetbrains.kotlin.android") version "2.0.21" apply false
}
