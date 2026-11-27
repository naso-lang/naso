package com.nasolang.intellij.highlighting

import com.intellij.openapi.editor.colors.TextAttributesKey
import com.intellij.openapi.editor.colors.TextAttributesKey.createTextAttributesKey
import com.intellij.openapi.editor.DefaultLanguageHighlighterColors
import com.intellij.openapi.fileTypes.SyntaxHighlighterBase
import com.intellij.psi.TokenType
import com.nasolang.intellij.lexer.NasoLexer
import com.nasolang.intellij.psi.NasoTypes
import org.jetbrains.annotations.NotNull

class NasoSyntaxHighlighter : SyntaxHighlighterBase() {
    private val tokenAttributes: Map<IElementType, TextAttributesKey> = mapOfEntries(
        // Keywords
        NasoTypes.FN to KEYWORD,
        NasoTypes.LET to KEYWORD,
        NasoTypes.MUT to KEYWORD,
        NasoTypes.CONST to KEYWORD,
        NasoTypes.STRUCT to KEYWORD,
        NasoTypes.ENUM to KEYWORD,
        NasoTypes.IMPL to KEYWORD,
        NasoTypes.TRAIT to KEYWORD,
        NasoTypes.TYPE to KEYWORD,
        NasoTypes.IF to KEYWORD,
        NasoTypes.ELSE to KEYWORD,
        NasoTypes.MATCH to KEYWORD,
        NasoTypes.LOOP to KEYWORD,
        NasoTypes.WHILE to KEYWORD,
        NasoTypes.FOR to KEYWORD,
        NasoTypes.BREAK to KEYWORD,
        NasoTypes.CONTINUE to KEYWORD,
        NasoTypes.RETURN to KEYWORD,
        NasoTypes.PUB to KEYWORD,
        NasoTypes.USE to KEYWORD,
        NasoTypes.MOD to KEYWORD,
        NasoTypes.AS to KEYWORD,
        NasoTypes.WHERE to KEYWORD,
        NasoTypes.ASYNC to KEYWORD,
        NasoTypes.AWAIT to KEYWORD,
        NasoTypes.MOVE to KEYWORD,
        
        // Quantum keywords
        NasoTypes.CIRCUIT to QUANTUM_KEYWORD,
        NasoTypes.QUBITS to QUANTUM_KEYWORD,
        NasoTypes.GATE to QUANTUM_KEYWORD,
        NasoTypes.MEASURE to QUANTUM_KEYWORD,
        NasoTypes.QALLOC to QUANTUM_KEYWORD,
        NasoTypes.QFREE to QUANTUM_KEYWORD,
        
        // Quantum gates
        NasoTypes.H to QUANTUM_GATE,
        NasoTypes.X to QUANTUM_GATE,
        NasoTypes.Y to QUANTUM_GATE,
        NasoTypes.Z to QUANTUM_GATE,
        NasoTypes.CNOT to QUANTUM_GATE,
        NasoTypes.CZ to QUANTUM_GATE,
        NasoTypes.RX to QUANTUM_GATE,
        NasoTypes.RY to QUANTUM_GATE,
        NasoTypes.RZ to QUANTUM_GATE,
        NasoTypes.S to QUANTUM_GATE,
        NasoTypes.T to QUANTUM_GATE,
        
        // Literals
        TokenType.INTEGER to NUMBER,
        TokenType.FLOAT to NUMBER,
        TokenType.STRING_LITERAL to STRING,
        TokenType.CHARACTER_LITERAL to STRING,
        NasoTypes.TRUE to CONSTANT,
        NasoTypes.FALSE to CONSTANT,
        NasoTypes.NONE to CONSTANT,
        NasoTypes.SOME to CONSTANT,
        
        // Comments
        TokenType.COMMENT to COMMENT,
        TokenType.WHITE_SPACE to WHITESPACE,
        
        // Operators
        TokenType.OPERATION to OPERATOR,
        
        // Types
        NasoTypes.PRIMITIVE_TYPE to TYPE,
        NasoTypes.REFERENCE_TYPE to TYPE,
        NasoTypes.MUTABLE_REFERENCE_TYPE to TYPE,
        NasoTypes.ARRAY_TYPE to TYPE,
        NasoTypes.TUPLE_TYPE to TYPE,
        NasoTypes.FUNCTION_TYPE to TYPE,
        NasoTypes.GENERIC_TYPE to TYPE,
        NasoTypes.QUANTUM_TYPE to QUANTUM_TYPE,
        
        // Identifiers
        TokenType.IDENTIFIER to IDENTIFIER
    )

    @NotNull
    override fun getTokenHighlights(tokenType: IElementType): Array<TextAttributesKey> {
        return tokenAttributes[tokenType]?.let { arrayOf(it) } ?: emptyArray()
    }
    
    @NotNull
    override fun getHighlightingLexer() = NasoLexer.create()
}

companion object {
    val KEYWORD = createTextAttributesKey("NASO_KEYWORD", DefaultLanguageHighlighterColors.KEYWORD)
    val QUANTUM_KEYWORD = createTextAttributesKey("NASO_QUANTUM_KEYWORD", DefaultLanguageHighlighterColors.KEYWORD)
    val QUANTUM_GATE = createTextAttributesKey("NASO_QUANTUM_GATE", DefaultLanguageHighlighterColors.METHOD_CALL)
    val NUMBER = createTextAttributesKey("NASO_NUMBER", DefaultLanguageHighlighterColors.NUMBER)
    val STRING = createTextAttributesKey("NASO_STRING", DefaultLanguageHighlighterColors.STRING)
    val CONSTANT = createTextAttributesKey("NASO_CONSTANT", DefaultLanguageHighlighterColors.CONSTANT)
    val COMMENT = createTextAttributesKey("NASO_COMMENT", DefaultLanguageHighlighterColors.COMMENT)
    val WHITESPACE = createTextAttributesKey("NASO_WHITESPACE", DefaultLanguageHighlighterColors.WHITESPACE)
    val OPERATOR = createTextAttributesKey("NASO_OPERATOR", DefaultLanguageHighlighterColors.OPERATION_SIGN)
    val TYPE = createTextAttributesKey("NASO_TYPE", DefaultLanguageHighlighterColors.CLASS_NAME)
    val QUANTUM_TYPE = createTextAttributesKey("NASO_QUANTUM_TYPE", DefaultLanguageHighlighterColors.INTERFACE_NAME)
    val IDENTIFIER = createTextAttributesKey("NASO_IDENTIFIER", DefaultLanguageHighlighterColors.IDENTIFIER)
}