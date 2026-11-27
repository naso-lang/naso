package com.nasolang.intellij.actions

import com.intellij.openapi.actionSystem.AnAction
import com.intellij.openapi.actionSystem.AnActionEvent
import com.intellij.openapi.actionSystem.CommonDataKeys
import com.intellij.openapi.project.Project
import com.intellij.openapi.vfs.VirtualFile
import com.intellij.openapi.ui.Messages
import com.nasolang.intellij.runConfig.NasoRunConfiguration

class NasoBuildAction : AnAction() {
    override fun actionPerformed(e: AnActionEvent) {
        val project = e.getProject() ?: return
        val file = e.getData(CommonDataKeys.VIRTUAL_FILE) ?: return
        
        runNasoCommand(project, "build", file.path ?: "")
    }
}

class NasoRunAction : AnAction() {
    override fun actionPerformed(e: AnActionEvent) {
        val project = e.getProject() ?: return
        val file = e.getData(CommonDataKeys.VIRTUAL_FILE) ?: return
        
        runNasoCommand(project, "run", file.path ?: "")
    }
}

class NasoTestAction : AnAction() {
    override fun actionPerformed(e: AnActionEvent) {
        val project = e.getProject() ?: return
        
        runNasoCommand(project, "test", "")
    }
}

class NasoCheckAction : AnAction() {
    override fun actionPerformed(e: AnActionEvent) {
        val project = e.getProject() ?: return
        val file = e.getData(CommonDataKeys.VIRTUAL_FILE) ?: return
        
        runNasoCommand(project, "check", file.path ?: "")
    }
}

class NasoFmtAction : AnAction() {
    override fun actionPerformed(e: AnActionEvent) {
        val project = e.getProject() ?: return
        val file = e.getData(CommonDataKeys.VIRTUAL_FILE) ?: return
        
        runNasoCommand(project, "fmt", file.path ?: "")
    }
}

class NasoQuantumVisualizeAction : AnAction() {
    override fun actionPerformed(e: AnActionEvent) {
        val project = e.getProject() ?: return
        val file = e.getData(CommonDataKeys.VIRTUAL_FILE) ?: return
        
        Messages.showInfoMessage(project, "Quantum circuit visualization would open for ${file.name}", "Quantum Visualization")
    }
}

class NasoQuantumSimulateAction : AnAction() {
    override fun actionPerformed(e: AnActionEvent) {
        val project = e.getProject() ?: return
        val file = e.getData(CommonDataKeys.VIRTUAL_FILE) ?: return
        
        Messages.showInfoMessage(project, "Quantum circuit simulation would run for ${file.name}", "Quantum Simulation")
    }
}

class NasoQuantumExportAction : AnAction() {
    override fun actionPerformed(e: AnActionEvent) {
        val project = e.getProject() ?: return
        val file = e.getData(CommonDataKeys.VIRTUAL_FILE) ?: return
        
        Messages.showInfoMessage(project, "Quantum circuit would be exported to OpenQASM for ${file.name}", "Export OpenQASM")
    }
}

class NasoPkgAddAction : AnAction() {
    override fun actionPerformed(e: AnActionEvent) {
        val project = e.getProject() ?: return
        
        val packageName = Messages.showInputDialog(project, "Enter package name:", "Add Package", Messages.getQuestionIcon())
        if (packageName != null && packageName.isNotBlank()) {
            runNasoCommand(project, "pkg add $packageName", "")
        }
    }
}

class NasoPkgInstallAction : AnAction() {
    override fun actionPerformed(e: AnActionEvent) {
        val project = e.getProject() ?: return
        
        runNasoCommand(project, "pkg install", "")
    }
}

class NasoPkgPublishAction : AnAction() {
    override fun actionPerformed(e: AnActionEvent) {
        val project = e.getProject() ?: return
        
        val confirm = Messages.showYesNoDialog(project, "Publish package to registry?", "Publish Package", Messages.getQuestionIcon())
        if (confirm == Messages.YES) {
            runNasoCommand(project, "pkg publish", "")
        }
    }
}

private fun runNasoCommand(project: Project, command: String, filePath: String) {
    val settings = com.nasolang.intellij.settings.NasoSettings.getInstance()
    val nasoPath = settings.nasoExecutablePath
    
    val cmd = mutableListOf(nasoPath)
    cmd.addAll(command.split(" "))
    
    if (filePath.isNotEmpty()) {
        cmd.add(filePath)
    }
    
    val processBuilder = ProcessBuilder(cmd)
    processBuilder.directory(java.io.File(project.basePath))
    
    try {
        val process = processBuilder.start()
        // Show output in tool window
        val output = process.inputStream.reader().readText()
        val error = process.errorStream.reader().readText()
        
        if (process.waitFor() == 0) {
            Messages.showInfoMessage(project, output, "Naso $command succeeded")
        } else {
            Messages.showErrorDialog(project, error, "Naso $command failed")
        }
    } catch (ex: Exception) {
        Messages.showErrorDialog(project, ex.message ?: "Unknown error", "Naso $command error")
    }
}