package com.nasolang.intellij.formatter

import com.intellij.formatting.*
import com.intellij.lang.ASTNode
import com.intellij.openapi.util.TextRange
import com.intellij.psi.PsiElement
import com.intellij.psi.codeStyle.CodeStyleSettings
import com.nasolang.intellij.psi.NasoTypes
import org.jetbrains.annotations.NotNull
import org.jetbrains.annotations.Nullable

class NasoFormattingModelBuilder : FormattingModelBuilder {
    @NotNull
    override fun createModel(
        @NotNull element: PsiElement,
        @NotNull settings: CodeStyleSettings
    ): FormattingModel {
        return FormattingModelProvider.createFormattingModelForPsiElement(element, settings, NasoBlock(element.node!!, settings))
    }
}

class NasoBlock(
    private val node: ASTNode,
    private val settings: CodeStyleSettings
) : Block {
    private val subBlocks: List<Block>
    private val indent: Indent
    private val alignment: Alignment?
    private val wrap: Wrap?
    private val spacing: SpacingBuilder

    init {
        val children = mutableListOf<Block>()
        val childNodes = node.children
        
        for (i in childNodes.indices) {
            val child = childNodes[i]
            val next = if (i + 1 < childNodes.size) childNodes[i + 1] else null
            children.add(NasoBlock(child, settings, next))
        }
        
        subBlocks = children
        
        indent = when (node.elementType) {
            NasoTypes.BLOCK_EXPR,
            NasoTypes.FUNCTION_DEF,
            NasoTypes.STRUCT_DEF,
            NasoTypes.ENUM_DEF,
            NasoTypes.IMPL_BLOCK,
            NasoTypes.TRAIT_DEF,
            NasoTypes.CIRCUIT_DEF,
            NasoTypes.IF_EXPR,
            NasoTypes.MATCH_EXPR,
            NasoTypes.LOOP_EXPR,
            NasoTypes.WHILE_EXPR,
            NasoTypes.FOR_EXPR -> Indent.getNormalIndent()
            else -> Indent.getNoneIndent()
        }
        
        alignment = null
        wrap = null
        
        spacing = SpacingBuilder()
        for (i in childNodes.indices) {
            val child = childNodes[i]
            val next = if (i + 1 < childNodes.size) childNodes[i + 1] else null
            if (next != null) {
                addSpacing(spacing, child, next)
            }
        }
    }

    private fun addSpacing(spacing: SpacingBuilder, first: ASTNode, second: ASTNode) {
        when {
            first.elementType == com.intellij.psi.TokenType.LBRACE && second.elementType != com.intellij.psi.TokenType.RBRACE -> {
                spacing.spaceIf(first, second, 0, 1, true)
            }
            first.elementType != com.intellij.psi.TokenType.LBRACE && second.elementType == com.intellij.psi.TokenType.RBRACE -> {
                spacing.spaceIf(first, second, 0, 1, true)
            }
            first.elementType == com.intellij.psi.TokenType.SEMICOLON -> {
                spacing.spaceIf(first, second, 0, 0, false)
            }
            first.elementType == com.intellij.psi.TokenType.COMMA -> {
                spacing.spaceIf(first, second, 0, 1, true)
            }
            first.elementType == com.intellij.psi.TokenType.COLON -> {
                spacing.spaceIf(first, second, 0, 1, false)
            }
            else -> {
                spacing.spaceIf(first, second, 1, 1, false)
            }
        }
    }

    @NotNull
    override fun getSubBlocks(): List<Block> = subBlocks

    @Nullable
    override fun getWrap(): Wrap? = wrap

    @Nullable
    override fun getIndent(): Indent = indent

    @Nullable
    override fun getAlignment(): Alignment? = alignment

    @Nullable
    override fun getSpacing(@Nullable child1: Block, @Nullable child2: Block): Spacing? = spacing.build()

    @NotNull
    override fun getNode(): ASTNode = node

    @Nullable
    override fun getChildAttributes(newChildIndex: Int): ChildAttributes {
        return ChildAttributes(Indent.getNormalIndent(), null)
    }

    @NotNull
    override fun getTextRange(): TextRange = node.textRange
}

class NasoLanguageCodeStyleSettingsProvider : com.intellij.psi.codeStyle.LanguageCodeStyleSettingsProvider {
    @NotNull
    override fun getLanguage(): com.intellij.lang.Language = com.nasolang.intellij.NasoLanguage.INSTANCE

    @NotNull
    override fun createCustomSettings(@NotNull settings: CodeStyleSettings): com.intellij.psi.codeStyle.CommonCodeStyleSettings {
        val custom = com.intellij.psi.codeStyle.CommonCodeStyleSettings(settings)
        custom.BRACE_STYLE = com.intellij.psi.codeStyle.CommonCodeStyleSettings.NEXT_LINE
        custom.SPACE_BEFORE_IF_PARENTHESES = true
        custom.SPACE_BEFORE_METHOD_CALL_PARENTHESES = false
        custom.SPACE_WITHIN_METHOD_CALL_PARENTHESES = false
        custom.SPACE_WITHIN_BRACKETS = false
        return custom
    }
}

class NasoCodeStyleSettingsProvider : com.intellij.psi.codeStyle.CodeStyleSettingsProvider {
    @NotNull
    override fun getSettings(): CodeStyleSettings {
        return CodeStyleSettings()
    }
}