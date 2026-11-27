package com.nasolang.intellij

import com.intellij.find.findUsages.FindUsagesProvider
import com.intellij.psi.PsiElement
import com.intellij.psi.PsiNamedElement
import com.nasolang.intellij.psi.*
import org.jetbrains.annotations.NotNull
import org.jetbrains.annotations.Nullable

class NasoFindUsagesProvider : FindUsagesProvider {
    override fun canFindUsagesFor(@NotNull element: PsiElement): Boolean {
        return element is PsiNamedElement && element.language.isKindOf(NasoLanguage.INSTANCE)
    }

    @NotNull
    override fun getType(@NotNull element: PsiElement): String {
        return when (element) {
            is NasoFunctionDefinition -> "function"
            is NasoStructDefinition -> "struct"
            is NasoEnumDefinition -> "enum"
            is NasoTraitDefinition -> "trait"
            is NasoTypeAlias -> "type alias"
            is NasoLetBinding -> "variable"
            is NasoConstDefinition -> "constant"
            is NasoCircuitDefinition -> "circuit"
            is NasoGateApplication -> "gate"
            is NasoField -> "field"
            is NasoEnumVariant -> "enum variant"
            else -> "element"
        }
    }

    @NotNull
    override fun getDescriptiveName(@NotNull element: PsiElement): String {
        return element.name ?: "unknown"
    }

    @NotNull
    override fun getNodeText(@NotNull element: PsiElement, @NotNull useFullName: Boolean): String {
        return element.name ?: "unknown"
    }

    @Nullable
    override fun getHelpId(@NotNull element: PsiElement): String {
        return null
    }
}