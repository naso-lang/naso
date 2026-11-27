package com.nasolang.intellij.inspection

import com.intellij.codeInspection.*
import com.intellij.psi.PsiElement
import com.intellij.psi.PsiElementVisitor
import com.nasolang.intellij.psi.NasoTypes
import org.jetbrains.annotations.NotNull

class NasoQuantumCircuitInspection : LocalInspectionTool() {
    @NotNull
    override fun getGroupDisplayName(): String = "Naso"

    @NotNull
    override fun getDisplayName(): String = "Quantum Circuit Error"

    @NotNull
    override fun getShortName(): String = "NasoQuantumCircuitError"

    override fun isEnabledByDefault(): Boolean = true

    override fun getDefaultLevel(): com.intellij.codeHighlighting.HighlightDisplayLevel = 
        com.intellij.codeHighlighting.HighlightDisplayLevel.ERROR

    @NotNull
    override fun buildVisitor(@NotNull holder: ProblemsHolder, @NotNull isOnTheFly: Boolean): PsiElementVisitor {
        return object : PsiElementVisitor() {
            override fun visitElement(@NotNull element: PsiElement) {
                if (element.node?.elementType == NasoTypes.CIRCUIT_DEF) {
                    validateCircuit(element)
                }
                
                if (element.node?.elementType == NasoTypes.GATE_APPLICATION) {
                    validateGateApplication(element)
                }
                
                super.visitElement(element)
            }
        }
    }

    private fun validateCircuit(element: PsiElement) {
        val node = element.node ?: return
        
        // Check qubit count
        val qubitsNode = node.findChildByType(NasoTypes.QUBITS)
        val qubitCount = qubitsNode?.text?.toIntOrNull() ?: 0
        
        if (qubitCount <= 0) {
            holder.registerProblem(
                qubitsNode ?: element,
                "Circuit must have at least 1 qubit",
                ProblemHighlightType.ERROR
            )
        }
        
        if (qubitCount > 1000) {
            holder.registerProblem(
                qubitsNode ?: element,
                "Circuit has $qubitCount qubits, which may exceed simulator capacity",
                ProblemHighlightType.WARNING
            )
        }
        
        // Check for unused qubits
        val usedQubits = mutableSetOf<Int>()
        val gates = node.children.filter { it.elementType == NasoTypes.GATE_APPLICATION }
        for (gate in gates) {
            for (child in gate.children) {
                if (child.elementType == NasoTypes.INTEGER) {
                    usedQubits.add(child.text.toInt())
                }
            }
        }
        
        for (i in 0 until qubitCount) {
            if (i !in usedQubits) {
                holder.registerProblem(
                    element,
                    "Qubit $i is allocated but never used",
                    ProblemHighlightType.WARNING
                )
            }
        }
        
        // Check for out-of-bounds qubit access
        for (gate in gates) {
            for (child in gate.children) {
                if (child.elementType == NasoTypes.INTEGER) {
                    val qubit = child.text.toInt()
                    if (qubit >= qubitCount) {
                        holder.registerProblem(
                            child,
                            "Qubit index $qubit out of bounds (circuit has $qubitCount qubits)",
                            ProblemHighlightType.ERROR
                        )
                    }
                }
            }
        }
    }

    private fun validateGateApplication(element: PsiElement) {
        val node = element.node ?: return
        val gateName = node.findChildByType(NasoTypes.GATE)?.text
            ?: node.findChildByType(NasoTypes.H)?.text
            ?: node.findChildByType(NasoTypes.X)?.text
            ?: node.findChildByType(NasoTypes.Y)?.text
            ?: node.findChildByType(NasoTypes.Z)?.text
            ?: node.findChildByType(NasoTypes.CNOT)?.text
            ?: node.findChildByType(NasoTypes.CZ)?.text
            ?: node.findChildByType(NasoTypes.RX)?.text
            ?: node.findChildByType(NasoTypes.RY)?.text
            ?: node.findChildByType(NasoTypes.RZ)?.text
            ?: node.findChildByType(NasoTypes.S)?.text
            ?: node.findChildByType(NasoTypes.T)?.text
        
        val qubits = node.children.filter { it.elementType == NasoTypes.INTEGER }.map { it.text.toInt() }
        
        when (gateName?.uppercase()) {
            "CNOT", "CX" -> {
                if (qubits.size != 2) {
                    holder.registerProblem(
                        node,
                        "CNOT gate requires exactly 2 qubits (control, target), got ${qubits.size}",
                        ProblemHighlightType.ERROR
                    )
                }
            }
            "CZ" -> {
                if (qubits.size != 2) {
                    holder.registerProblem(
                        node,
                        "CZ gate requires exactly 2 qubits, got ${qubits.size}",
                        ProblemHighlightType.ERROR
                    )
                }
            }
            "RX", "RY", "RZ" -> {
                val params = node.children.filter { it.elementType == NasoTypes.FLOAT }
                if (params.size != 1) {
                    holder.registerProblem(
                        node,
                        "${gateName} gate requires exactly 1 angle parameter",
                        ProblemHighlightType.ERROR
                    )
                }
                if (qubits.size != 1) {
                    holder.registerProblem(
                        node,
                        "${gateName} gate requires exactly 1 qubit, got ${qubits.size}",
                        ProblemHighlightType.ERROR
                    )
                }
            }
            "H", "X", "Y", "Z", "S", "T" -> {
                if (qubits.size != 1) {
                    holder.registerProblem(
                        node,
                        "${gateName} gate requires exactly 1 qubit, got ${qubits.size}",
                        ProblemHighlightType.ERROR
                    )
                }
            }
        }
    }
}