package com.nasolang.intellij.completion

import com.intellij.codeInsight.completion.*
import com.intellij.codeInsight.lookup.LookupElementBuilder
import com.intellij.patterns.PlatformPatterns
import com.intellij.psi.PsiElement
import com.intellij.util.ProcessingContext
import com.nasolang.intellij.NasoLanguage
import com.nasolang.intellij.psi.NasoTypes
import org.jetbrains.annotations.NotNull

class NasoCompletionContributor : CompletionContributor() {
    init {
        extend(
            CompletionType.BASIC,
            PlatformPatterns.psiElement().withLanguage(NasoLanguage.INSTANCE),
            CompletionProvider { parameters, context, result ->
                addKeywordCompletions(result)
                addQuantumCompletions(result)
                addTypeCompletions(result)
            }
        )
    }

    private fun addKeywordCompletions(result: CompletionResultSet) {
        val keywords = listOf(
            "fn", "let", "mut", "const", "struct", "enum", "impl", "trait", "type",
            "if", "else", "match", "loop", "while", "for", "break", "continue", "return",
            "pub", "use", "mod", "as", "where", "async", "await", "move",
            "true", "false", "None", "Some"
        )
        
        for (kw in keywords) {
            result.addElement(LookupElementBuilder.create(kw).withBoldness(true))
        }
    }

    private fun addQuantumCompletions(result: CompletionResultSet) {
        val quantumKeywords = listOf(
            "circuit", "qubits", "gate", "measure", "qalloc", "qfree"
        )
        
        for (kw in quantumKeywords) {
            result.addElement(LookupElementBuilder.create(kw).withBoldness(true).withIcon(com.intellij.icons.AllIcons.Nodes.Keyword))
        }
        
        val gates = listOf(
            "H", "X", "Y", "Z", "CNOT", "CZ", "RX", "RY", "RZ", "S", "T"
        )
        
        for (gate in gates) {
            result.addElement(LookupElementBuilder.create(gate).withIcon(com.intellij.icons.AllIcons.Nodes.Method))
        }
    }

    private fun addTypeCompletions(result: CompletionResultSet) {
        val types = listOf(
            "i8", "i16", "i32", "i64", "i128",
            "u8", "u16", "u32", "u64", "u128",
            "f32", "f64",
            "bool", "char", "str", "String",
            "Qubit", "Circuit", "Result"
        )
        
        for (t in types) {
            result.addElement(LookupElementBuilder.create(t).withIcon(com.intellij.icons.AllIcons.Nodes.Class))
        }
    }
}