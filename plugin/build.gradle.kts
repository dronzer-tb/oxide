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

tasks.processResources {
    val props = mapOf(
        "version" to project.version.toString(),
        "apiVersion" to providers.gradleProperty("mcApiVersion").get()
    )
    inputs.properties(props)
    filesMatching("paper-plugin.yml") {
        expand(props)
    }
}

tasks.compileJava {
    options.encoding = "UTF-8"
}
