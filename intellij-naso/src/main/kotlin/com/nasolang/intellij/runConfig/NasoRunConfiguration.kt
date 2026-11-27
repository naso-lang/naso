package com.nasolang.intellij.runConfig

import com.intellij.execution.configurations.*
import com.intellij.execution.runners.ExecutionEnvironment
import com.intellij.openapi.options.SettingsEditor
import com.intellij.openapi.project.Project
import com.intellij.openapi.util.InvalidDataException
import com.intellij.openapi.util.WriteExternalException
import com.intellij.openapi.util.JDOMExternalizer
import org.jdom.Element
import org.jetbrains.annotations.NotNull
import org.jetbrains.annotations.Nullable

class NasoRunConfigurationType : ConfigurationType {
    @NotNull
    override fun getDisplayName(): String = "Naso Application"
    
    @NotNull
    override fun getConfigurationTypeDescription(): String = "Run Naso application"
    
    @NotNull
    override fun getId(): String = "NasoRunConfiguration"
    
    @Nullable
    override fun getIcon(): javax.swing.Icon? = com.intellij.icons.AllIcons.RunConfigurations.Application
    
    @NotNull
    override fun getConfigurationFactories(): Array<ConfigurationFactory> {
        return arrayOf(NasoRunConfigurationFactory(this))
    }
}

class NasoRunConfigurationFactory(configurationType: ConfigurationType) : ConfigurationFactory(configurationType) {
    @NotNull
    override fun createTemplateConfiguration(@NotNull project: Project): RunConfiguration {
        return NasoRunConfiguration(project, this, "")
    }
}

class NasoRunConfiguration(
    project: Project,
    factory: ConfigurationFactory,
    name: String
) : RunConfiguration(project, factory, name) {
    
    var mainFile: String = ""
    var programArgs: String = ""
    var workingDirectory: String = ""
    var buildTarget: String = "llvm"
    var releaseMode: Boolean = true
    var envVars: Map<String, String> = emptyMap()
    
    override fun getValidatedState(): RunConfigurationBase.ValidationResult {
        if (mainFile.isEmpty()) {
            return RunConfigurationBase.ValidationResult.error("Main file not specified")
        }
        return RunConfigurationBase.ValidationResult.success()
    }
    
    @NotNull
    override fun createRunProfileState(@NotNull environment: ExecutionEnvironment): RunnerState? {
        return NasoCommandLineState(environment, this)
    }
    
    override fun readExternal(element: Element) {
        super.readExternal(element)
        mainFile = JDOMExternalizer.readString(element, "mainFile")
        programArgs = JDOMExternalizer.readString(element, "programArgs")
        workingDirectory = JDOMExternalizer.readString(element, "workingDirectory")
        buildTarget = JDOMExternalizer.readString(element, "buildTarget", "llvm")
        releaseMode = JDOMExternalizer.readBoolean(element, "releaseMode", true)
        // envVars would be read from child elements
    }
    
    override fun writeExternal(element: Element) {
        super.writeExternal(element)
        JDOMExternalizer.writeString(element, "mainFile", mainFile)
        JDOMExternalizer.writeString(element, "programArgs", programArgs)
        JDOMExternalizer.writeString(element, "workingDirectory", workingDirectory)
        JDOMExternalizer.writeString(element, "buildTarget", buildTarget)
        JDOMExternalizer.writeBoolean(element, "releaseMode", releaseMode)
        // envVars would be written as child elements
    }
}

class NasoCommandLineState(
    environment: ExecutionEnvironment,
    configuration: NasoRunConfiguration
) : com.intellij.execution.runners.ExecutionEnvironment.ExecutionResult(environment) {
    
    override fun startProcess(): com.intellij.execution.process.ProcessHandler? {
        val command = buildCommand(configuration)
        val processHandler = com.intellij.execution.process.ProcessHandler()
        // Would start actual process with command
        return processHandler
    }
    
    private fun buildCommand(config: NasoRunConfiguration): List<String> {
        val cmd = mutableListOf("naso", "run")
        
        if (config.buildTarget != "llvm") {
            cmd.add("--target")
            cmd.add(config.buildTarget)
        }
        
        if (config.releaseMode) {
            cmd.add("--release")
        }
        
        if (config.workingDirectory.isNotEmpty()) {
            // Would set working directory
        }
        
        cmd.add(config.mainFile)
        
        if (config.programArgs.isNotEmpty()) {
            cmd.add("--")
            cmd.addAll(config.programArgs.split(" "))
        }
        
        return cmd
    }
}