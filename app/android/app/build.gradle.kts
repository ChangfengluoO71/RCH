import java.io.FileInputStream
import java.util.Properties

plugins {
    id("com.android.application")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

// 正式签名：本地读取 android/key.properties（gitignore，不入库），CI 通过环境变量注入；
// 两者都缺失时回退 debug 签名，保证本地 flutter run --release 仍然可用。
val keystoreProperties = Properties()
val keystorePropertiesFile = rootProject.file("key.properties")
if (keystorePropertiesFile.exists()) {
    keystoreProperties.load(FileInputStream(keystorePropertiesFile))
}

fun propOrEnv(key: String, env: String): String? =
    keystoreProperties.getProperty(key) ?: System.getenv(env)

android {
    namespace = "com.rch.reader"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    defaultConfig {
        applicationId = "com.rch.reader"
        // You can update the following values to match your application needs.
        // For more information, see: https://flutter.dev/to/review-gradle-config.
        minSdk = flutter.minSdkVersion
        targetSdk = flutter.targetSdkVersion
        versionCode = flutter.versionCode
        versionName = flutter.versionName
    }

    signingConfigs {
        create("release") {
            val storeFile = propOrEnv("storeFile", "RELEASE_STORE_FILE")
            val storePassword = propOrEnv("storePassword", "RELEASE_STORE_PASSWORD")
            val keyAlias = propOrEnv("keyAlias", "RELEASE_KEY_ALIAS")
            val keyPassword = propOrEnv("keyPassword", "RELEASE_KEY_PASSWORD")
            if (storeFile != null && storePassword != null && keyAlias != null && keyPassword != null) {
                // key.properties 位于 android/ 根目录，需相对 rootProject 解析。
                this.storeFile = rootProject.file(storeFile)
                this.storePassword = storePassword
                this.keyAlias = keyAlias
                this.keyPassword = keyPassword
            }
        }
    }

    buildTypes {
        release {
            // 有正式签名配置时用它；否则回退 debug 签名。
            signingConfig = signingConfigs.findByName("release")?.takeIf { it.storeFile != null }
                ?: signingConfigs.getByName("debug")
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

flutter {
    source = "../.."
}
