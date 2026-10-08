// The Android shell around the engine.
//
// This project produces only the *delivery vehicle*: the Rust library in
// `crates/kintsugi-android` is the engine, and `app/src/main/jniLibs/` is
// where `cargo ndk` drops one `.so` per ABI. See `platforms/android/README.md`
// for the exact commands.
plugins {
    id("com.android.application") version "8.7.3"
    id("org.jetbrains.kotlin.android") version "2.0.21"
}

android {
    namespace = "com.kintsugi.engine"
    compileSdk = 35

    defaultConfig {
        applicationId = "com.kintsugi.engine"
        minSdk = 24
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"

        ndk {
            // The ABIs CI builds with cargo-ndk. Keeping the list here means
            // the APK never ships a slice with no library behind it.
            abiFilters += listOf("arm64-v8a", "armeabi-v7a", "x86_64", "x86")
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
            // Release signing needs a keystore from CI secrets; the debug
            // build is signed automatically and installs on any device.
            proguardFiles(
                getDefaultProguardFile("proguard-android-optimize.txt"),
                "proguard-rules.pro",
            )
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    sourceSets["main"].jniLibs.srcDirs("src/main/jniLibs")
}

dependencies {
    implementation("androidx.appcompat:appcompat:1.7.0")
}
