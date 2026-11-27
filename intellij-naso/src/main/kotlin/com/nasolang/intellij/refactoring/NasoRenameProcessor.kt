package com.nasolang.intellij.refactoring

import com.intellij.refactoring.rename.RenamePsiElementProcessor
import com.intellij.psi.PsiElement
import com.intellij.psi.PsiNamedElement
import com.nasolang.intellij.psi.*
import org.jetbrains.annotations.NotNull
import org.jetbrains.annotations.Nullable

class NasoRenameProcessor : RenamePsiElementProcessor() {
    override fun canProcessElement(@NotNull element: PsiElement): Boolean {
        return element is PsiNamedElement && element.language.isKindOf(com.nasolang.intellij.NasoLanguage.INSTANCE)
    }

    @NotNull
    override fun getAllRenames(
        @NotNull element: PsiElement,
        @NotNull newName: String
    ): Array<com.intellij.refactoring.rename.RenameUtil.RenameInfo> {
        // Delegate to default implementation
        return super.getAllRenames(element, newName)
    }

    @Nullable
    override fun substituteElementToRename(@NotNull element: PsiElement, @NotNull contextEditor: com.intellij.openapi.editor.Editor): PsiElement {
        return when (element) {
            is NasoFunctionDefinition,
                 is NasoStructDefinition,
                 is NasoEnumDefinition,
                 is NasoTraitDefinition,
                 is NasoTypeAlias,
                 is NasoLetBinding,
                 is NasoConstDefinition,
                 is NasoCircuitDefinition,
                 is NasoGateApplication,
                 is NasoField,
                 is NasoEnumVariant -> element
            else -> super.substituteElementToRename(element, contextEditor)
        }
    }

    override fun prepareRenaming(element: PsiElement, newName: String, allRenames: MutableList<com.intellij.usageView.UsageInfo>): Boolean {
        // Add validation for Naso-specific naming rules
        if (!isValidNasoName(newName)) {
            com.intellij.openapi.ui.Messages.showErrorDialog(
                "Invalid Naso identifier: $newName. Must start with letter or underscore, contain only alphanumeric and underscore.",
                "Rename Error"
            )
            return false
        }
        return true
    }

    private fun isValidNasoName(name: String): Boolean {
        if (name.isEmpty()) return false
        val first = name[0]
        if (!first.isLetter() && first != '_') return false
        return name.all { it.isLetterOrDigit() || it == '_' }
    }
}