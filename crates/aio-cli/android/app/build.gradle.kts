// Made by `ax build --android`: the app's Rust library (src/main/jniLibs)
// in a small Java activity around the system WebView. No other dependency.
plugins {
    id("com.android.application")
}

android {
    namespace = "aio.rt"
    compileSdk = 35

    defaultConfig {
        applicationId = "{{APPLICATION_ID}}"
        minSdk = 29 // Android 10: the oldest whose WebView still gets updates
        targetSdk = 35
        versionCode = {{VERSION_CODE}}
        versionName = "{{VERSION}}"
    }

    // One APK per CPU type, each within the size budget.
    splits {
        abi {
            isEnable = {{SPLITS}}
            reset()
            include({{ABIS}})
            isUniversalApk = {{UNIVERSAL}}
        }
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    // The library deflated: half the APK (Android unpacks it at install).
    packaging {
        jniLibs {
            useLegacyPackaging = true
        }
    }
}
