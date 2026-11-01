plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "ai.storyteller.pdfcraft"
    compileSdk = 35

    defaultConfig {
        applicationId = "ai.storyteller.pdfcraft"
        // `cargo xtask mobile android` builds the native library for arm64-v8a, armeabi-v7a and
        // x86_64 (xtask/src/mobile.rs); minSdk 26 is JNA's and UniFFI's own floor.
        minSdk = 26
        targetSdk = 35
        versionCode = 1
        versionName = "0.1.0"
    }

    buildFeatures {
        compose = true
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    kotlinOptions {
        jvmTarget = "17"
    }

    packaging {
        // Only one build's .so per ABI should ever land here (cargo xtask mobile android writes
        // them fresh under src/main/jniLibs/<abi>/); this just stops a stale duplicate from a
        // library this app doesn't otherwise pull in from failing the merge.
        jniLibs.pickFirsts += "**/libpdfcraft_mobile_ffi.so"
    }
}

dependencies {
    implementation(platform("androidx.compose:compose-bom:2025.01.00"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.material3:material3")
    implementation("androidx.compose.ui:ui-tooling-preview")
    implementation("androidx.activity:activity-compose:1.10.0")
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.7")
    implementation("androidx.lifecycle:lifecycle-runtime-ktx:2.8.7")
    implementation("org.jetbrains.kotlinx:kotlinx-coroutines-android:1.9.0")
    // The UniFFI-generated Kotlin bindings (app/src/main/java/uniffi/pdfcraft_mobile_ffi/, written
    // by `cargo xtask mobile android`) call into the native library through JNA.
    implementation("net.java.dev.jna:jna:5.15.0@aar")
}
