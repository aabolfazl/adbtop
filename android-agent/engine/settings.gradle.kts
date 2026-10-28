// The engine is also a standalone build so it can be tested without the Android SDK:
//   ./gradlew -p engine test
pluginManagement {
    repositories {
        gradlePluginPortal()
        mavenCentral()
    }
}

dependencyResolutionManagement {
    repositories {
        mavenCentral()
    }
}

rootProject.name = "engine"
