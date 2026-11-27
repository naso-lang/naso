package com.nasolang.intellij.parser

import com.intellij.lang.ParserDefinition
import com.intellij.lang.PsiParser
import com.intellij.lang.ASTNode
import com.intellij.lang.Language
import com.intellij.lexer.Lexer
import com.intellij.openapi.project.Project
import com.intellij.psi.FileViewProvider
import com.intellij.psi.PsiElement
import com.intellij.psi.PsiFile
import com.intellij.psi.tree.IFileElementType
import com.intellij.psi.tree.TokenSet
import com.nasolang.intellij.NasoLanguage
import com.nasolang.intellij.lexer.NasoLexer
import org.jetbrains.annotations.NotNull

class NasoParserDefinition : ParserDefinition {
    companion object {
        val FILE = IFileElementType(NasoLanguage.INSTANCE)
        val KEYWORDS = TokenSet.create(
            com.nasolang.intellij.psi.NasoTypes.FN,
            com.nasolang.intellij.psi.NasoTypes.LET,
            com.nasolang.intellij.psi.NasoTypes.MUT,
            com.nasolang.intellij.psi.NasoTypes.CONST,
            com.nasolang.intellij.psi.NasoTypes.STRUCT,
            com.nasolang.intellij.psi.NasoTypes.ENUM,
            com.nasolang.intellij.psi.NasoTypes.IMPL,
            com.nasolang.intellij.psi.NasoTypes.TRAIT,
            com.nasolang.intellij.psi.NasoTypes.TYPE,
            com.nasolang.intellij.psi.NasoTypes.IF,
            com.nasolang.intellij.psi.NasoTypes.ELSE,
            com.nasolang.intellij.psi.NasoTypes.MATCH,
            com.nasolang.intellij.psi.NasoTypes.LOOP,
            com.nasolang.intellij.psi.NasoTypes.WHILE,
            com.nasolang.intellij.psi.NasoTypes.FOR,
            com.nasolang.intellij.psi.NasoTypes.BREAK,
            com.nasolang.intellij.psi.NasoTypes.CONTINUE,
            com.nasolang.intellij.psi.NasoTypes.RETURN,
            com.nasolang.intellij.psi.NasoTypes.PUB,
            com.nasolang.intellij.psi.NasoTypes.USE,
            com.nasolang.intellij.psi.NasoTypes.MOD,
            com.nasolang.intellij.psi.NasoTypes.AS,
            com.nasolang.intellij.psi.NasoTypes.WHERE,
            com.nasolang.intellij.psi.NasoTypes.ASYNC,
            com.nasolang.intellij.psi.NasoTypes.AWAIT,
            com.nasolang.intellij.psi.NasoTypes.MOVE,
            com.nasolang.intellij.psi.NasoTypes.CIRCUIT,
            com.nasolang.intellij.psi.NasoTypes.QUBITS,
            com.nasolang.intellij.psi.NasoTypes.GATE,
            com.nasolang.intellij.psi.NasoTypes.MEASURE,
            com.nasolang.intellij.psi.NasoTypes.QALLOC,
            com.nasolang.intellij.psi.NasoTypes.QFREE
        )
    }

    override fun getFileNodeType(): IFileElementType = FILE

    @NotNull
    override fun createLexer(@NotNull project: Project): Lexer = NasoLexer.create()

    @NotNull
    override fun createParser(@NotNull project: Project): PsiParser = NasoParser()

    override fun getWhitespaceTokens(): TokenSet = TokenSet.create(com.intellij.psi.TokenType.WHITE_SPACE)

    override fun getCommentTokens(): TokenSet = TokenSet.create(com.intellij.psi.TokenType.COMMENT)

    override fun getStringLiteralElements(): TokenSet = TokenSet.create(com.intellij.psi.TokenType.STRING_LITERAL)

    @NotNull
    override fun createElement(@NotNull node: ASTNode): PsiElement = com.nasolang.intellij.psi.NasoElementFactory.createElement(node)

    @NotNull
    override fun createFile(@NotNull viewProvider: FileViewProvider): PsiFile = com.nasolang.intellij.psi.NasoFile(viewProvider)
}