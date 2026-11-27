package com.nasolang.intellij

import com.intellij.lang.documentation.DocumentationProvider
import com.intellij.lang.documentation.DocumentationMarkup
import com.intellij.psi.PsiElement
import com.intellij.psi.PsiManager
import com.nasolang.intellij.psi.*
import org.jetbrains.annotations.NotNull
import org.jetbrains.annotations.Nullable

class NasoDocumentationProvider : DocumentationProvider() {
    @Nullable
    override fun generateDoc(element: PsiElement, originalElement: PsiElement?): String? {
        return when (element) {
            is NasoFunctionDefinition -> generateFunctionDoc(element)
            is NasoStructDefinition -> generateStructDoc(element)
            is NasoEnumDefinition -> generateEnumDoc(element)
            is NasoTraitDefinition -> generateTraitDoc(element)
            is NasoCircuitDefinition -> generateCircuitDoc(element)
            is NasoGateApplication -> generateGateDoc(element)
            else -> null
        }
    }

    private fun generateFunctionDoc(fn: NasoFunctionDefinition): String {
        val name = fn.getName() ?: "unknown"
        val returnType = fn.getReturnType() ?: "()"
        return DocumentationMarkup.DEFINITION_START + "fn $name() -> $returnType" + DocumentationMarkup.DEFINITION_END +
            "\n\n" + DocumentationMarkup.CONTENT_START +
            "Function defined in Naso." +
            DocumentationMarkup.CONTENT_END
    }

    private fun generateStructDoc(struct: NasoStructDefinition): String {
        val name = struct.getName() ?: "unknown"
        val fields = struct.getFields().joinToString(", ") { "${it.getName()}: ${it.getType()}" }
        return DocumentationMarkup.DEFINITION_START + "struct $name { $fields }" + DocumentationMarkup.DEFINITION_END +
            "\n\n" + DocumentationMarkup.CONTENT_START +
            "Struct with ${struct.getFields().size} fields." +
            DocumentationMarkup.CONTENT_END
    }

    private fun generateEnumDoc(enum: NasoEnumDefinition): String {
        val name = enum.getName() ?: "unknown"
        val variants = enum.getVariants().joinToString(", ") { it.getName() ?: "?" }
        return DocumentationMarkup.DEFINITION_START + "enum $name { $variants }" + DocumentationMarkup.DEFINITION_END +
            "\n\n" + DocumentationMarkup.CONTENT_START +
            "Enum with ${enum.getVariants().size} variants." +
            DocumentationMarkup.CONTENT_END
    }

    private fun generateTraitDoc(trait: NasoTraitDefinition): String {
        val name = trait.getName() ?: "unknown"
        return DocumentationMarkup.DEFINITION_START + "trait $name" + DocumentationMarkup.DEFINITION_END +
            "\n\n" + DocumentationMarkup.CONTENT_START +
            "Trait definition." +
            DocumentationMarkup.CONTENT_END
    }

    private fun generateCircuitDoc(circuit: NasoCircuitDefinition): String {
        val name = circuit.getName() ?: "unknown"
        val qubits = circuit.getQubitCount()
        val gates = circuit.getGates().size
        return DocumentationMarkup.DEFINITION_START + "circuit $name { qubits: $qubits }" + DocumentationMarkup.DEFINITION_END +
            "\n\n" + DocumentationMarkup.CONTENT_START +
            "Quantum circuit with $qubits qubits and $gates gates." +
            DocumentationMarkup.CONTENT_END
    }

    private fun generateGateDoc(gate: NasoGateApplication): String {
        val name = gate.getGateName() ?: "unknown"
        val qubits = gate.getQubits().joinToString(", ")
        return DocumentationMarkup.DEFINITION_START + "$name $qubits" + DocumentationMarkup.DEFINITION_END +
            "\n\n" + DocumentationMarkup.CONTENT_START +
            "Quantum gate applied to qubits [$qubits]." +
            DocumentationMarkup.CONTENT_END
    }

    @Nullable
    override fun getDocumentationElementForLink(@NotNull context: PsiElement, @NotNull link: String, @NotNull project: com.intellij.openapi.project.Project): PsiElement? {
        return null
    }

    @Nullable
    override fun getDocumentationElementForLookupItem(@NotNull context: PsiManager, @NotNull element: com.intellij.codeInsight.lookup.LookupElement): PsiElement? {
        return null
    }
}