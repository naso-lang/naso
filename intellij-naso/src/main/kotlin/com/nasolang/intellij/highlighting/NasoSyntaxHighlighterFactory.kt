package com.nasolang.intellij.highlighting

import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.openapi.editor.colors.TextAttributesKey.createTextAttributesKey
import com.intellij.openapi.editor.DefaultLanguageHighlighterColors
import com.intellij.openapi.fileTypes.SyntaxHighlighterFactory
import com.nasolang.intellij.NasoLanguage
import org.jetbrains.annotations.NotNull

class NasoSyntaxHighlighterFactory : SyntaxHighlighterFactory {
    @NotNull
    override fun getSyntaxHighlighter(): com.intellij.openapi.fileTypes.SyntaxHighlighter {
        return com.nasolang.intellij.highlighting.NasoSyntaxHighlighter()
    }
}