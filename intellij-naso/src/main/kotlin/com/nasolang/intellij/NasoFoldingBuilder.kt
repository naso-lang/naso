package com.nasolang.intellij

import com.intellij.lang.ASTNode
import com.intellij.lang.folding.FoldingBuilderEx
import com.intellij.lang.folding.FoldingDescriptor
import com.intellij.openapi.editor.Document
import com.intellij.openapi.util.TextRange
import com.intellij.psi.PsiElement
import com.intellij.psi.util.PsiTreeUtil
import com.nasolang.intellij.psi.NasoTypes
import org.jetbrains.annotations.NotNull
import org.jetbrains.annotations.Nullable

class NasoFoldingBuilder : FoldingBuilderEx() {
    @NotNull
    override fun buildFoldRegions(
        @NotNull node: ASTNode,
        @NotNull document: Document,
        @NotNull quick: Boolean
    ): Array<FoldingDescriptor> {
        val descriptors = mutableListOf<FoldingDescriptor>()
        collectFoldRegions(node, descriptors)
        return descriptors.toTypedArray()
    }

    private fun collectFoldRegions(node: ASTNode, descriptors: MutableList<FoldingDescriptor>) {
        when (node.elementType) {
            NasoTypes.FUNCTION_DEF,
            NasoTypes.STRUCT_DEF,
            NasoTypes.ENUM_DEF,
            NasoTypes.IMPL_BLOCK,
            NasoTypes.TRAIT_DEF,
            NasoTypes.CIRCUIT_DEF,
            NasoTypes.BLOCK_EXPR,
            NasoTypes.IF_EXPR,
            NasoTypes.MATCH_EXPR,
            NasoTypes.LOOP_EXPR,
            NasoTypes.WHILE_EXPR,
            NasoTypes.FOR_EXPR -> {
                val children = node.children
                for (child in children) {
                    if (child.elementType == com.intellij.psi.TokenType.LBRACE) {
                        val endBrace = findMatchingBrace(node, child)
                        if (endBrace != null) {
                            val range = TextRange.create(child.startOffset, endBrace.endOffset)
                            if (range.length > 2) {
                                descriptors.add(FoldingDescriptor(node, range))
                            }
                        }
                        break
                    }
                }
            }
        }
        
        for (child in node.children) {
            collectFoldRegions(child, descriptors)
        }
    }

    private fun findMatchingBrace(node: ASTNode, openBrace: ASTNode): ASTNode? {
        var depth = 0
        for (child in node.children) {
            if (child.startOffset >= openBrace.startOffset) {
                if (child.elementType == com.intellij.psi.TokenType.LBRACE) {
                    depth++
                } else if (child.elementType == com.intellij.psi.TokenType.RBRACE) {
                    depth--
                    if (depth == 0) {
                        return child
                    }
                }
            }
        }
        return null
    }

    @Nullable
    override fun getPlaceholderText(@NotNull node: ASTNode): String {
        return when (node.elementType) {
            NasoTypes.FUNCTION_DEF -> "{...}"
            NasoTypes.STRUCT_DEF -> "{...}"
            NasoTypes.ENUM_DEF -> "{...}"
            NasoTypes.IMPL_BLOCK -> "{...}"
            NasoTypes.TRAIT_DEF -> "{...}"
            NasoTypes.CIRCUIT_DEF -> "{...}"
            else -> "{...}"
        }
    }

    override fun isCollapsedByDefault(@NotNull node: ASTNode): Boolean {
        return node.elementType in setOf(
            NasoTypes.FUNCTION_DEF,
            NasoTypes.STRUCT_DEF,
            NasoTypes.ENUM_DEF,
            NasoTypes.IMPL_BLOCK,
            NasoTypes.TRAIT_DEF,
            NasoTypes.CIRCUIT_DEF
        )
    }
}