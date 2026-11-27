package com.nasolang.intellij

import com.intellij.lang.PairedBraceMatcher
import com.intellij.psi.tree.TokenSet
import com.nasolang.intellij.psi.NasoTypes
import org.jetbrains.annotations.NotNull
import org.jetbrains.annotations.Nullable

class NasoBraceMatcher : PairedBraceMatcher {
    private val openingBraces = TokenSet.create(
        com.intellij.psi.TokenType.LBRACE,
        com.intellij.psi.TokenType.LPARENTH,
        com.intellij.psi.TokenType.LBRACKET
    )
    
    private val closingBraces = TokenSet.create(
        com.intellij.psi.TokenType.RBRACE,
        com.intellij.psi.TokenType.RPARENTH,
        com.intellij.psi.TokenType.RBRACKET
    )

    @NotNull
    override fun getOpeningBraces(): TokenSet = openingBraces

    @NotNull
    override fun getClosingBraces(): TokenSet = closingBraces

    @Nullable
    override fun isPairedBraces(
        @NotNull opening: IElementType,
        @NotNull closing: IElementType
    ): Boolean {
        return when (opening) {
            com.intellij.psi.TokenType.LBRACE -> closing == com.intellij.psi.TokenType.RBRACE
            com.intellij.psi.TokenType.LPARENTH -> closing == com.intellij.psi.TokenType.RPARENTH
            com.intellij.psi.TokenType.LBRACKET -> closing == com.intellij.psi.TokenType.RBRACKET
            else -> false
        }
    }

    @Nullable
    override fun getCodeConstructStart(
        @NotNull file: com.intellij.openapi.fileTypes.FileViewProvider,
        @NotNull openingBraceOffset: Int
    ): Int? {
        return null
    }
}