package com.nasolang.intellij.settings

import com.intellij.openapi.application.ApplicationManager
import com.intellij.openapi.components.Service
import com.intellij.openapi.components.State
import com.intellij.openapi.components.Storage
import com.intellij.openapi.components.StoragePathMacros
import org.jetbrains.annotations.NotNull

@Service
@State(
    name = "NasoSettings",
    storages = [Storage(StoragePathMacros.APP_CONFIG + "/naso.settings.xml")]
)
class NasoSettings : com.intellij.openapi.components.PersistentStateComponent<NasoSettings.State> {
    var nasoExecutablePath: String = "naso"
    var lspServerPath: String = "naso-lsp"
    var enableQuantumVisualization: Boolean = true
    var quantumSimulator: String = "statevector"
    var autoFormatOnSave: Boolean = true
    var enableSMTDiagnostics: Boolean = true
    
    class State {
        var nasoExecutablePath: String = "naso"
        var lspServerPath: String = "naso-lsp"
        var enableQuantumVisualization: Boolean = true
        var quantumSimulator: String = "statevector"
        var autoFormatOnSave: Boolean = true
        var enableSMTDiagnostics: Boolean = true
    }
    
    @NotNull
    override fun getState(): State = this
    
    override fun loadState(state: State) {
        nasoExecutablePath = state.nasoExecutablePath
        lspServerPath = state.lspServerPath
        enableQuantumVisualization = state.enableQuantumVisualization
        quantumSimulator = state.quantumSimulator
        autoFormatOnSave = state.autoFormatOnSave
        enableSMTDiagnostics = state.enableSMTDiagnostics
    }
    
    companion object {
        fun getInstance(): NasoSettings = ApplicationManager.getApplication().getService(NasoSettings::class.java)
    }
}