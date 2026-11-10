import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

android {
    namespace = "fr.mawena.jarvis"
    compileSdk = 36
    buildToolsVersion = "35.0.0"
    // Sert à dépouiller la bibliothèque Rust à l'empaquetage.
    ndkVersion = "28.2.13676358"

    defaultConfig {
        applicationId = "fr.mawena.jarvis"
        // Pixel 4 XL : Android 13, son dernier système.
        minSdk = 33
        targetSdk = 33
        versionCode = 3
        versionName = "0.3.0"
        ndk {
            abiFilters += "arm64-v8a"
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = false
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    sourceSets["main"].apply {
        // Liaisons Kotlin générées par uniffi-bindgen et bibliothèque Rust compilée par
        // cargo-ndk (scripts/android-build.sh).
        java.srcDir("build/generated/uniffi")
        jniLibs.srcDir("build/rustJniLibs")
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

dependencies {
    // Seule dépendance d'exécution du Kotlin généré par UniFFI.
    implementation("net.java.dev.jna:jna:5.17.0@aar")
    // Lecture du QR code de `jarvis pair` (zxing core seulement, sans AndroidX).
    implementation("com.journeyapps:zxing-android-embedded:4.3.0")
    testImplementation("junit:junit:4.13.2")
}
