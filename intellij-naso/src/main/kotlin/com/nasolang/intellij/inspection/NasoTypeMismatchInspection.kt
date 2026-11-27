package com.nasolang.intellij.inspection

import com.intellij.codeInspection.*
import com.intellij.psi.PsiElement
import com.intellij.psi.PsiElementVisitor
import com.nasolang.intellij.psi.NasoTypes
import org.jetbrains.annotations.NotNull

class NasoTypeMismatchInspection : LocalInspectionTool() {
    @NotNull
    override fun getGroupDisplayName(): String = "Naso"

    @NotNull
    override fun getDisplayName(): String = "Type Mismatch"

    @NotNull
    override fun getShortName(): String = "NasoTypeMismatch"

    override fun isEnabledByDefault(): Boolean = true

    override fun getDefaultLevel(): com.intellij.codeHighlighting.HighlightDisplayLevel = 
        com.intellij.codeHighlighting.HighlightDisplayLevel.ERROR

    @NotNull
    override fun buildVisitor(@NotNull holder: ProblemsHolder, @NotNull isOnTheFly: Boolean): PsiElementVisitor {
        return object : PsiElementVisitor() {
            override fun visitElement(@NotNull element: PsiElement) {
                // Check binary expressions for type compatibility
                if (element.node?.elementType == NasoTypes.BINARY_EXPR) {
                    val left = element.node?.findChildByType(NasoTypes.BINARY_EXPR)?.children?.firstOrNull()
                    val right = element.node?.findChildByType(NasoTypes.BINARY_EXPR)?.children?.lastOrNull()
                    
                    if (left != null && right != null) {
                        val leftType = inferType(left)
                        val rightType = inferType(right)
                        
                        if (leftType != null && rightType != null && leftType != rightType) {
                            holder.registerProblem(
                                element,
                                "Type mismatch: expected $leftType, found $rightType",
                                ProblemHighlightType.ERROR
                            )
                        }
                    }
                }
                
                // Check function call arguments
                if (element.node?.elementType == NasoTypes.CALL_EXPR) {
                    // Would check argument types against function signature
                }
                
                super.visitElement(element)
            }
        }
    }

    private fun inferType(node: ASTNode): String? {
        // Simplified type inference
        return when (node.elementType) {
            NasoTypes.INTEGER -> "i64"
            NasoTypes.FLOAT -> "f64"
            NasoTypes.STRING_LITERAL -> "String"
            NasoTypes.TRUE, NasoTypes.FALSE -> "bool"
            NasoTypes.CALL_EXPR -> "unknown"
            else -> null
        }
    }
}