plugins {
    id("com.android.application")
    id("org.jetbrains.kotlin.android")
    id("org.jetbrains.kotlin.plugin.compose")
}

android {
    namespace = "com.pocketwebcam.app"
    compileSdk = 36

    defaultConfig {
        applicationId = "com.pocketwebcam.app"
        // 29 (Android 10): the first with TLS 1.3, which protocol v2 requires.
        minSdk = 29
        targetSdk = 36
        versionCode = 3
        versionName = "0.2.0"
    }

    // Upload key for Google Play. Path and passwords live in ~/.gradle/gradle.properties
    // (never in the repo). Without them, release builds are simply left unsigned.
    val uploadStore = providers.gradleProperty("POCKETWEBCAM_UPLOAD_STORE_FILE").orNull
    signingConfigs {
        if (uploadStore != null) {
            create("upload") {
                storeFile = file(uploadStore)
                storePassword = providers.gradleProperty("POCKETWEBCAM_UPLOAD_STORE_PASSWORD").get()
                keyAlias = providers.gradleProperty("POCKETWEBCAM_UPLOAD_KEY_ALIAS").get()
                keyPassword = providers.gradleProperty("POCKETWEBCAM_UPLOAD_KEY_PASSWORD").get()
            }
        }
    }

    buildTypes {
        release {
            isMinifyEnabled = true
            isShrinkResources = true
            proguardFiles(getDefaultProguardFile("proguard-android-optimize.txt"), "proguard-rules.pro")
            if (uploadStore != null) signingConfig = signingConfigs.getByName("upload")
        }
    }
    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
    kotlinOptions {
        jvmTarget = "17"
    }
    buildFeatures {
        compose = true
        buildConfig = true
    }
}

dependencies {
    implementation("androidx.core:core-ktx:1.15.0")
    implementation("androidx.activity:activity-compose:1.9.3")
    implementation("androidx.lifecycle:lifecycle-viewmodel-compose:2.8.7")
    implementation("androidx.lifecycle:lifecycle-runtime-compose:2.8.7")
    implementation(platform("androidx.compose:compose-bom:2024.12.01"))
    implementation("androidx.compose.ui:ui")
    implementation("androidx.compose.foundation:foundation")
    implementation("androidx.compose.material3:material3")

    testImplementation("junit:junit:4.13.2")
    // The real org.json, since android.jar only has stubs on the JVM.
    testImplementation("org.json:json:20260814")
}
