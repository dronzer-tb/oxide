plugins {
    `java-library`
}

group = "dev.oxide"
version = "1.0.0"

java {
    toolchain {
        languageVersion.set(JavaLanguageVersion.of(22))
    }
}

repositories {
    mavenCentral()
    maven("https://maven.fabricmc.net/")
}

dependencies {
    compileOnly("org.slf4j:slf4j-api:2.0.12")
    testImplementation("org.junit.jupiter:junit-jupiter:5.10.2")
}

tasks.jar {
    manifest {
        attributes(
            "Fabric-Loom-Version" to "1.0",
            "Specification-Title" to "oxide-client",
            "Specification-Vendor" to "x00f8",
            "Specification-Version" to project.version,
            "Implementation-Title" to project.name,
            "Implementation-Version" to project.version
        )
    }
}
