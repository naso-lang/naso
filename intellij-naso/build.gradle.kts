plugins {
    id 'org.jetbrains.intellij' version '1.17.0'
    id 'org.jetbrains.kotlin.jvm' version '1.9.20'
    id 'org.jetbrains.changelog' version '1.1.0'
}

group = 'com.nasolang'
version = '0.1.0'

repositories {
    mavenCentral()
    maven { url 'https://maven.pkg.jetbrains.space/public/p/intellij/intellij-dependencies/maven' }
}

dependencies {
    implementation 'org.jetbrains.kotlin:kotlin-stdlib-jdk8:1.9.20'
    implementation 'org.jetbrains.intellij:platform-2023.3.4'
    testImplementation 'org.jetbrains.kotlin:kotlin-test-junit:1.9.20'
    testImplementation 'org.jetbrains.intellij:test-framework:2023.3.4'
}

intellij {
    version.set('2023.3.4')
    type.set('IC') // IntelliJ Community Edition
    plugins.set(listOf('Kotlin'))
    downloadSources.set(true)
    sandboxDirectory.set(file('build/sandbox'))
}

tasks.withType<org.jetbrains.kotlin.gradle.tasks.KotlinCompile> {
    kotlinOptions {
        jvmTarget = '17'
        freeCompilerArgs = listOf('-Xjsr305=strict')
    }
}

tasks.named('test') {
    useJUnitPlatform()
}

changelog {
    version.set(project.version.toString())
}