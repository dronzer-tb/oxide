plugins {
    java
}

group = "dev.oxide"
version = providers.gradleProperty("pluginVersion").get()

java {
    toolchain {
        languageVersion.set(
            JavaLanguageVersion.of(providers.gradleProperty("javaVersion").get().toInt())
        )
    }
}

repositories {
    mavenCentral()
    // PaperMC's repo hosts both the Paper API and the Folia API
    // (dev.folia:folia-api) — Folia is a Paper fork and publishes its API
    // artifact alongside Paper's in the same Nexus instance.
    maven("https://repo.papermc.io/repository/maven-public/")
}

// Deliberately a Gradle property, not a literal here: see gradle.properties
// for why — nobody has confirmed a Folia build for MC 26.2 yet.
val foliaApiVersion: String = providers.gradleProperty("foliaApiVersion").get()

dependencies {
    // compileOnly: the Folia server jar provides this API at runtime; the
    // plugin must not shade or bundle it.
    compileOnly("dev.folia:folia-api:$foliaApiVersion")

    testImplementation(platform("org.junit:junit-bom:5.10.2"))
    testImplementation("org.junit.jupiter:junit-jupiter")
    // Gradle 9's test executor needs the launcher on the runtime classpath
    // explicitly; junit-jupiter no longer pulls it in transitively.
    testRuntimeOnly("org.junit.platform:junit-platform-launcher")
}

tasks.test {
    useJUnitPlatform()
    // Wired for CI, deliberately not invoked locally — see repo-wide rule:
    // heavy/verifying builds run in GitHub Actions, not on this machine.
}

// The compiled oxide-ffi cdylib to embed in the jar. Defaults to where a workspace-root
// `cargo build --release -p oxide-ffi` puts it, which is what CI runs immediately before
// this build; override with -PnativeLibraryFile=/abs/path for a locally built one.
// Deliberately not a hard requirement: the provenance-overlay half of this plugin works
// without any native library, so a missing .so warns and produces a jar without one rather
// than failing the build. GeneratorService then errors only if /oxide createworld is used.
val nativeLibraryFile: File = providers.gradleProperty("nativeLibraryFile")
    .map { file(it) }
    .getOrElse(rootDir.resolve("../target/release/liboxide_ffi.so"))

tasks.processResources {
    val props = mapOf(
        "version" to project.version.toString(),
        "apiVersion" to providers.gradleProperty("mcApiVersion").get()
    )
    inputs.properties(props)
    filesMatching("paper-plugin.yml") {
        expand(props)
    }

    if (nativeLibraryFile.isFile) {
        from(nativeLibraryFile) {
            into("natives/linux-x86_64")
        }
    } else {
        logger.warn(
            "oxide: no native library at ${nativeLibraryFile.absolutePath} -- building a jar " +
                "WITHOUT an embedded liboxide_ffi.so; /oxide createworld will fail on it " +
                "unless native-library-path in config.yml points at a real .so"
        )
    }
}

tasks.compileJava {
    options.encoding = "UTF-8"
}
