import org.jetbrains.kotlin.gradle.dsl.JvmTarget

plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
}

val adbtopVersion: String = rootDir.resolve("../Cargo.toml").readLines()
    .first { it.startsWith("version = ") }
    .substringAfter('"')
    .substringBefore('"')

fun versionCodeOf(version: String): Int {
    val parts = version.substringBefore('-').substringBefore('+').split('.').map { it.toIntOrNull() ?: 0 }
    return parts.getOrElse(0) { 0 } * 10_000 + parts.getOrElse(1) { 0 } * 100 + parts.getOrElse(2) { 0 }
}

val keystore: String? = System.getenv("ADBTOP_KEYSTORE")

android {
    namespace = "io.github.aabolfazl.perfagent"
    compileSdk = 35

    defaultConfig {
        applicationId = "io.github.aabolfazl.perfagent"
        // getConnectionOwnerUid and DnsResolver need Android 10.
        minSdk = 29
        targetSdk = 35
        versionCode = versionCodeOf(adbtopVersion)
        versionName = adbtopVersion
    }

    signingConfigs {
        if (keystore != null) {
            create("release") {
                storeFile = file(keystore)
                storePassword = System.getenv("ADBTOP_KEYSTORE_PASSWORD")
                keyAlias = System.getenv("ADBTOP_KEY_ALIAS") ?: "adbtop"
                keyPassword = System.getenv("ADBTOP_KEY_PASSWORD") ?: System.getenv("ADBTOP_KEYSTORE_PASSWORD")
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            if (keystore != null) {
                signingConfig = signingConfigs.getByName("release")
            }
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

kotlin {
    compilerOptions {
        jvmTarget.set(JvmTarget.JVM_17)
    }
}

dependencies {
    implementation("io.github.aabolfazl.perfagent:engine:0.1.0")
}
