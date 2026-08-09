allprojects {
    repositories {
        // 国内/本机镜像默认关闭：CI（GitHub Actions）直接走官方仓库，避免阿里云
        // 镜像故障（如 502 Bad Gateway）连锁禁用所有仓库导致构建失败。
        // 本地构建如需加速，在 ~/.gradle/gradle.properties 设置 rch.aliyun.mirror=true，
        // 或设置环境变量 RCH_ALIYUN_MIRROR=true。
        val useLocalMirrors =
            providers.gradleProperty("rch.aliyun.mirror").orNull == "true" ||
                System.getenv("RCH_ALIYUN_MIRROR") == "true"
        if (useLocalMirrors) {
            maven { url = uri("file:///D:/Temp/local-maven") }
            maven { url = uri("https://maven.aliyun.com/repository/google") }
            maven { url = uri("https://maven.aliyun.com/repository/central") }
        }
        google()
        mavenCentral()
    }
}

val newBuildDir: Directory =
    rootProject.layout.buildDirectory
        .dir("../../build")
        .get()
rootProject.layout.buildDirectory.value(newBuildDir)

subprojects {
    val newSubprojectBuildDir: Directory = newBuildDir.dir(project.name)
    project.layout.buildDirectory.value(newSubprojectBuildDir)
}
subprojects {
    project.evaluationDependsOn(":app")
}

tasks.register<Delete>("clean") {
    delete(rootProject.layout.buildDirectory)
}
