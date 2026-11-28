plugins {
    alias(libs.plugins.android.library)
}

// linkOut names the folder with the generated Kotlin (kotlin/) and the cargo-ndk output (jniLibs/).
val linkOut = file(providers.gradleProperty("linkOut").get())

android {
    namespace = "app.alazy.linktests"
    compileSdk = 37

    defaultConfig {
        minSdk = 26
        testInstrumentationRunner = "androidx.test.runner.AndroidJUnitRunner"
    }

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }
}

androidComponents {
    onVariants { variant ->
        variant.sources.kotlin?.addStaticSourceDirectory(linkOut.resolve("kotlin").path)
        variant.sources.jniLibs?.addStaticSourceDirectory(linkOut.resolve("jniLibs").path)
    }
}

dependencies {
    implementation("${libs.jna.get().module}:${libs.versions.jna.get()}@aar")
    implementation(libs.coroutines.android)

    androidTestImplementation(libs.androidx.test.runner)
    androidTestImplementation(libs.androidx.test.ext.junit)
}
