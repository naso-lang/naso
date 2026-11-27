package com.nasolang.intellij.lexer

import com.intellij.lexer.FlexAdapter
import com.intellij.lexer.Lexer
import com.nasolang.intellij.NasoLanguage
import org.jetbrains.annotations.NotNull

class NasoLexer : FlexAdapter(NasoLexerImpl(NasoLanguage.INSTANCE))

class NasoLexerImpl private constructor(language: com.intellij.lang.Language) : 
    com.intellij.lexer.LexerBase() {
    
    private val keywords = setOf(
        "fn", "let", "mut", "const", "struct", "enum", "impl", "trait", "type",
        "if", "else", "match", "loop", "while", "for", "break", "continue", "return",
        "pub", "use", "mod", "as", "where", "async", "await", "move",
        "circuit", "qubits", "gate", "measure", "qalloc", "qfree",
        "H", "X", "Y", "Z", "CNOT", "CZ", "RX", "RY", "RZ", "S", "T",
        "true", "false", "None", "Some"
    )
    
    private val operators = setOf(
        "+", "-", "*", "/", "%", "**",
        "==", "!=", "<", ">", "<=", ">=",
        "&&", "||", "!",
        "=", "+=", "-=", "*=", "/=", "%=",
        "->", "=>", "::", ":", ";", ",", ".", "..", "...",
        "|>", "<|", "@", "#", "$", "?", "??"
    )
    
    private var buffer: String = ""
    private var pos = 0
    private var tokenStart = 0
    private var tokenEnd = 0
    private var tokenType: com.intellij.psi.TokenType? = null
    
    override fun start(@NotNull buffer: CharSequence, startOffset: Int, endOffset: Int, initialState: Int) {
        this.buffer = buffer.toString()
        this.pos = startOffset
        this.tokenStart = startOffset
        this.tokenEnd = startOffset
    }
    
    override fun getTokenStart(): Int = tokenStart
    override fun getTokenEnd(): Int = tokenEnd
    override fun getTokenType(): com.intellij.psi.TokenType? = tokenType
    
    override fun advance(): Int {
        if (pos >= buffer.length) return com.intellij.psi.TokenType.EOF
        
        tokenStart = pos
        val char = buffer[pos]
        
        when {
            char.isWhitespace() -> scanWhitespace()
            char.isDigit() -> scanNumber()
            char.isLetter() || char == '_' -> scanIdentifier()
            char == '"' -> scanString()
            char == '/' && pos + 1 < buffer.length && buffer[pos + 1] == '/' -> scanLineComment()
            char == '/' && pos + 1 < buffer.length && buffer[pos + 1] == '*' -> scanBlockComment()
            else -> scanOperatorOrPunctuation()
        }
        
        tokenEnd = pos
        return tokenType?.ordinal ?: com.intellij.psi.TokenType.BAD_CHARACTER.ordinal
    }
    
    private fun scanWhitespace() {
        while (pos < buffer.length && buffer[pos].isWhitespace()) pos++
        tokenType = com.intellij.psi.TokenType.WHITE_SPACE
    }
    
    private fun scanNumber() {
        val start = pos
        var hasDot = false
        var hasExp = false
        
        while (pos < buffer.length) {
            val c = buffer[pos]
            when {
                c.isDigit() -> pos++
                c == '.' && !hasDot && !hasExp -> { hasDot = true; pos++ }
                (c == 'e' || c == 'E') && !hasExp -> { hasExp = true; pos++ }
                (c == '+' || c == '-') && hasExp && pos > 0 && (buffer[pos - 1] == 'e' || buffer[pos - 1] == 'E') -> pos++
                else -> break
            }
        }
        
        tokenType = if (hasDot || hasExp) com.intellij.psi.TokenType.FLOAT else com.intellij.psi.TokenType.INTEGER
    }
    
    private fun scanIdentifier() {
        while (pos < buffer.length && (buffer[pos].isLetterOrDigit() || buffer[pos] == '_')) pos++
        val text = buffer.substring(tokenStart, pos)
        tokenType = if (text in keywords) com.intellij.psi.TokenType.KEYWORD else com.intellij.psi.TokenType.IDENTIFIER
    }
    
    private fun scanString() {
        pos++ // skip opening quote
        while (pos < buffer.length && buffer[pos] != '"') {
            if (buffer[pos] == '\\' && pos + 1 < buffer.length) pos += 2 else pos++
        }
        if (pos < buffer.length) pos++ // skip closing quote
        tokenType = com.intellij.psi.TokenType.STRING_LITERAL
    }
    
    private fun scanLineComment() {
        while (pos < buffer.length && buffer[pos] != '\n') pos++
        tokenType = com.intellij.psi.TokenType.COMMENT
    }
    
    private fun scanBlockComment() {
        pos += 2 // skip /*
        var depth = 1
        while (pos < buffer.length && depth > 0) {
            if (buffer[pos] == '*' && pos + 1 < buffer.length && buffer[pos + 1] == '/') {
                pos += 2
                depth--
            } else if (buffer[pos] == '/' && pos + 1 < buffer.length && buffer[pos + 1] == '*') {
                pos += 2
                depth++
            } else {
                pos++
            }
        }
        tokenType = com.intellij.psi.TokenType.COMMENT
    }
    
    private fun scanOperatorOrPunctuation() {
        // Try multi-character operators first
        for (len in 3 downTo 1) {
            if (pos + len <= buffer.length) {
                val candidate = buffer.substring(pos, pos + len)
                if (candidate in operators) {
                    pos += len
                    tokenType = com.intellij.psi.TokenType.OPERATION
                    return
                }
            }
        }
        
        // Single character
        pos++
        tokenType = com.intellij.psi.TokenType.OPERATION
    }
    
    override fun getState(): Int = 0
    override fun getBufferSequence(): CharSequence = buffer
    override fun getBufferEnd(): Int = buffer.length
    
    companion object {
        @JvmStatic
        fun create(): Lexer = NasoLexer()
    }
}