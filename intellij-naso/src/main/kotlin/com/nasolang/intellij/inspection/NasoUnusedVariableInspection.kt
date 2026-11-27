package com.nasolang.intellij.inspection

import com.intellij.codeInspection.*
import com.intellij.psi.PsiElement
import com.intellij.psi.PsiElementVisitor
import com.nasolang.intellij.psi.NasoTypes
import org.jetbrains.annotations.NotNull

class NasoUnusedVariableInspection : LocalInspectionTool() {
    @NotNull
    override fun getGroupDisplayName(): String = "Naso"

    @NotNull
    override fun getDisplayName(): String = "Unused Variable"

    @NotNull
    override fun getShortName(): String = "NasoUnusedVariable"

    override fun isEnabledByDefault(): Boolean = true

    override fun getDefaultLevel(): com.intellij.codeHighlighting.HighlightDisplayLevel = 
        com.intellij.codeHighlighting.HighlightDisplayLevel.WARNING

    @NotNull
    override fun buildVisitor(@NotNull holder: ProblemsHolder, @NotNull isOnTheFly: Boolean): PsiElementVisitor {
        return object : PsiElementVisitor() {
            override fun visitElement(@NotNull element: PsiElement) {
                if (element.node?.elementType == NasoTypes.LET_BINDING) {
                    checkUnusedLetBinding(element)
                }
                
                super.visitElement(element)
            }
        }
    }

    private fun checkUnusedLetBinding(element: PsiElement) {
        val node = element.node ?: return
        val nameNode = node.findChildByType(NasoTypes.IDENTIFIER) ?: return
        val name = nameNode.text
        
        // Check if variable name starts with underscore (intentionally unused)
        if (name.startsWith("_")) return
        
        // Find all references to this variable in the containing scope
        val containingBlock = findContainingBlock(element)
        if (containingBlock == null) return
        
        var isUsed = false
        collectReferences(containingBlock, name) { isUsed = true }
        
        if (!isUsed) {
            holder.registerProblem(
                nameNode,
                "Unused variable: $name",
                ProblemHighlightType.WARNING
            )
        }
    }

    private fun findContainingBlock(element: PsiElement): ASTNode? {
        var current = element.node?.parent
        while (current != null) {
            if (current.elementType in setOf(
                NasoTypes.BLOCK_EXPR,
                NasoTypes.FUNCTION_DEF,
                NasoTypes.IF_EXPR,
                NasoTypes.MATCH_EXPR,
                NasoTypes.LOOP_EXPR,
                NasoTypes.WHILE_EXPR,
                NasoTypes.FOR_EXPR
            )) {
                return current
            }
            current = current.parent
        }
        return null
    }

    private fun collectReferences(node: ASTNode, name: String, onFound: () -> Unit) {
        if (node.elementType == NasoTypes.IDENTIFIER && node.text == name) {
            onFound()
            return
        }
        
        for (child in node.children) {
            collectReferences(child, name, onFound)
        }
    }
}