package com.nasolang.intellij.settings

import com.intellij.openapi.components.Service
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.openapi.components.StoragePathMacros
import com.intellij.openapi.project.Project
import org.jetbrains.annotations.NotNull

@Service
@State(
    name = "NasoProjectSettings",
    storages = [Storage(StoragePathMacros.PROJECT_CONFIG_DIR + "/naso.project.settings.xml")]
)
class NasoProjectSettings : com.intellij.openapi.components.PersistentStateComponent<NasoProjectSettings.State> {
    var projectNasoExecutable: String = ""
    var projectBuildTarget: String = "llvm"
    var projectReleaseMode: Boolean = true
    var projectQuantumSimulator: String = "statevector"
    var customEnvVars: Map<String, String> = emptyMap()
    
    class State {
        var projectNasoExecutable: String = ""
        var projectBuildTarget: String = "llvm"
        var projectReleaseMode: Boolean = true
        var projectQuantumSimulator: String = "statevector"
        var customEnvVars: MutableMap<String, String> = mutableMapOf()
    }
    
    @NotNull
    override fun getState(): State = this
    
    override fun loadState(state: State) {
        projectNasoExecutable = state.projectNasoExecutable
        projectBuildTarget = state.projectBuildTarget
        projectReleaseMode = state.projectReleaseMode
        projectQuantumSimulator = state.projectQuantumSimulator
        customEnvVars = state.customEnvVars.toMap()
    }
    
    companion object {
        fun getInstance(project: Project): NasoProjectSettings = project.getService(NasoProjectSettings::class.java)
    }
}