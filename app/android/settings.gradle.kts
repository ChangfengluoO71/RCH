pluginManagement {
    val flutterSdkPath =
        run {
            val properties = java.util.Properties()
            file("local.properties").inputStream().use { properties.load(it) }
            val flutterSdkPath = properties.getProperty("flutter.sdk")
            require(flutterSdkPath != null) { "flutter.sdk not set in local.properties" }
            flutterSdkPath
        }

    includeBuild("$flutterSdkPath/packages/flutter_tools/gradle")

    repositories {
        // 阿里云镜像仅在本机显式开启时启用（见 build.gradle.kts 顶部注释），
        // CI 默认直接走 google() / mavenCentral() / gradlePluginPortal()。
        val useAliyunMirror =
            providers.gradleProperty("rch.aliyun.mirror").orNull == "true" ||
                System.getenv("RCH_ALIYUN_MIRROR") == "true"
        if (useAliyunMirror) {
            maven {
                url = uri("https://maven.aliyun.com/repository/google")
                content {
                    includeGroupByRegex("com\\.android.*")
                    includeGroupByRegex("com\\.google.*")
                    includeGroupByRegex("androidx.*")
                }
            }
            maven {
                url = uri("https://maven.aliyun.com/repository/gradle-plugin")
                content {
                    includeGroupByRegex("org\\.jetbrains\\.kotlin.*")
                    includeGroupByRegex("org\\.gradle.*")
                    includeGroupByRegex("dev\\.flutter.*")
                }
            }
            maven {
                url = uri("https://maven.aliyun.com/repository/central")
                content {
                    includeGroupByRegex("org\\.jetbrains.*")
                    includeGroupByRegex("org\\.kotlin.*")
                }
            }
        }
        google()
        mavenCentral()
        gradlePluginPortal()
    }
}

plugins {
    id("dev.flutter.flutter-plugin-loader") version "1.0.0"
    id("com.android.application") version "9.0.1" apply false
    id("org.jetbrains.kotlin.android") version "2.3.20" apply false
}

include(":app")
