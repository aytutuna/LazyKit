plugins {
    alias(libs.plugins.android.library)
}

// The artifact has to be the version of the crate in Cargo.lock, or the native side calls into the wrong classes.
val verifierVersion = file("../../../../rust/Cargo.lock").readLines().let { lines ->
    val at = lines.indexOfFirst { it.trim() == "name = \"rustls-platform-verifier-android\"" }
    lines.drop(at + 1).first { it.startsWith("version = ") }.substringAfter('"').substringBefore('"')
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
    implementation("org.rustls:rustls-platform-verifier:$verifierVersion")

    androidTestImplementation(libs.androidx.test.runner)
    androidTestImplementation(libs.androidx.test.ext.junit)
}
