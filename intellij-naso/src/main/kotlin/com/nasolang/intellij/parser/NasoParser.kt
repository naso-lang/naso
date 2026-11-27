package com.nasolang.intellij.parser

import com.intellij.lang.PsiBuilder
import com.intellij.lang.PsiParser
import com.intellij.psi.tree.IElementType
import com.nasolang.intellij.psi.NasoTypes
import org.jetbrains.annotations.NotNull

class NasoParser : PsiParser {
    @NotNull
    override fun parse(
        @NotNull root: IElementType,
        @NotNull builder: PsiBuilder
    ): ASTNode {
        val marker = builder.mark()
        
        while (!builder.eof()) {
            parseTopLevel(builder)
        }
        
        marker.done(root)
        return builder.getTreeBuilt()
    }
    
    private fun parseTopLevel(builder: PsiBuilder): Boolean {
        return when {
            parseFunction(builder) -> true
            parseStruct(builder) -> true
            parseEnum(builder) -> true
            parseImpl(builder) -> true
            parseTrait(builder) -> true
            parseTypeAlias(builder) -> true
            parseLetBinding(builder) -> true
            parseConst(builder) -> true
            parseCircuit(builder) -> true
            parseUse(builder) -> true
            parseMod(builder) -> true
            else -> {
                builder.advanceLexer()
                true
            }
        }
    }
    
    private fun parseFunction(builder: PsiBuilder): Boolean {
        if (!builder.getTokenType().equals(NasoTypes.FN)) return false
        
        val marker = builder.mark()
        builder.advanceLexer() // fn
        
        // Optional pub
        if (builder.getTokenType().equals(NasoTypes.PUB)) {
            builder.advanceLexer()
        }
        
        // Name
        if (builder.getTokenType().equals(NasoTypes.IDENTIFIER)) {
            builder.advanceLexer()
        } else {
            marker.error("Expected function name")
        }
        
        // Generics
        if (builder.getTokenType().toString() == "<") {
            parseGenerics(builder)
        }
        
        // Parameters
        if (builder.getTokenType().toString() == "(") {
            parseParameters(builder)
        }
        
        // Return type
        if (builder.getTokenType().toString() == "->") {
            builder.advanceLexer()
            parseType(builder)
        }
        
        // Where clause
        if (builder.getTokenType().equals(NasoTypes.WHERE)) {
            parseWhereClause(builder)
        }
        
        // Body
        if (builder.getTokenType().toString() == "{") {
            parseBlock(builder)
        } else if (builder.getTokenType().equals(NasoTypes.ASYNC)) {
            builder.advanceLexer()
            if (builder.getTokenType().toString() == "{") {
                parseBlock(builder)
            }
        } else {
            marker.error("Expected function body")
        }
        
        marker.done(NasoTypes.FUNCTION_DEF)
        return true
    }
    
    private fun parseStruct(builder: PsiBuilder): Boolean {
        if (!builder.getTokenType().equals(NasoTypes.STRUCT)) return false
        
        val marker = builder.mark()
        builder.advanceLexer() // struct
        
        if (builder.getTokenType().equals(NasoTypes.IDENTIFIER)) {
            builder.advanceLexer()
        } else {
            marker.error("Expected struct name")
        }
        
        if (builder.getTokenType().toString() == "<") {
            parseGenerics(builder)
        }
        
        if (builder.getTokenType().toString() == "{") {
            parseBlock(builder)
        } else {
            marker.error("Expected struct body")
        }
        
        marker.done(NasoTypes.STRUCT_DEF)
        return true
    }
    
    private fun parseEnum(builder: PsiBuilder): Boolean {
        if (!builder.getTokenType().equals(NasoTypes.ENUM)) return false
        
        val marker = builder.mark()
        builder.advanceLexer() // enum
        
        if (builder.getTokenType().equals(NasoTypes.IDENTIFIER)) {
            builder.advanceLexer()
        } else {
            marker.error("Expected enum name")
        }
        
        if (builder.getTokenType().toString() == "<") {
            parseGenerics(builder)
        }
        
        if (builder.getTokenType().toString() == "{") {
            parseBlock(builder)
        } else {
            marker.error("Expected enum body")
        }
        
        marker.done(NasoTypes.ENUM_DEF)
        return true
    }
    
    private fun parseImpl(builder: PsiBuilder): Boolean {
        if (!builder.getTokenType().equals(NasoTypes.IMPL)) return false
        
        val marker = builder.mark()
        builder.advanceLexer() // impl
        
        // Optional trait name
        if (builder.getTokenType().equals(NasoTypes.IDENTIFIER) && 
            builder.lookAhead(1)?.toString() == "for") {
            builder.advanceLexer() // trait name
            if (builder.getTokenType().toString() == "for") {
                builder.advanceLexer() // for
            }
        }
        
        // Type name
        if (builder.getTokenType().equals(NasoTypes.IDENTIFIER)) {
            builder.advanceLexer()
        }
        
        if (builder.getTokenType().toString() == "{") {
            parseBlock(builder)
        } else {
            marker.error("Expected impl body")
        }
        
        marker.done(NasoTypes.IMPL_BLOCK)
        return true
    }
    
    private fun parseTrait(builder: PsiBuilder): Boolean {
        if (!builder.getTokenType().equals(NasoTypes.TRAIT)) return false
        
        val marker = builder.mark()
        builder.advanceLexer() // trait
        
        if (builder.getTokenType().equals(NasoTypes.IDENTIFIER)) {
            builder.advanceLexer()
        } else {
            marker.error("Expected trait name")
        }
        
        if (builder.getTokenType().toString() == "<") {
            parseGenerics(builder)
        }
        
        if (builder.getTokenType().toString() == "{") {
            parseBlock(builder)
        } else {
            marker.error("Expected trait body")
        }
        
        marker.done(NasoTypes.TRAIT_DEF)
        return true
    }
    
    private fun parseTypeAlias(builder: PsiBuilder): Boolean {
        if (!builder.getTokenType().equals(NasoTypes.TYPE)) return false
        
        val marker = builder.mark()
        builder.advanceLexer() // type
        
        if (builder.getTokenType().equals(NasoTypes.IDENTIFIER)) {
            builder.advanceLexer()
        } else {
            marker.error("Expected type alias name")
        }
        
        if (builder.getTokenType().toString() == "<") {
            parseGenerics(builder)
        }
        
        if (builder.getTokenType().toString() == "=") {
            builder.advanceLexer()
            parseType(builder)
        }
        
        marker.done(NasoTypes.TYPE_ALIAS)
        return true
    }
    
    private fun parseLetBinding(builder: PsiBuilder): Boolean {
        if (!builder.getTokenType().equals(NasoTypes.LET)) return false
        
        val marker = builder.mark()
        builder.advanceLexer() // let
        
        if (builder.getTokenType().equals(NasoTypes.MUT)) {
            builder.advanceLexer()
        }
        
        if (builder.getTokenType().equals(NasoTypes.IDENTIFIER)) {
            builder.advanceLexer()
        } else {
            marker.error("Expected variable name")
        }
        
        if (builder.getTokenType().toString() == ":") {
            builder.advanceLexer()
            parseType(builder)
        }
        
        if (builder.getTokenType().toString() == "=") {
            builder.advanceLexer()
            parseExpression(builder)
        }
        
        marker.done(NasoTypes.LET_BINDING)
        return true
    }
    
    private fun parseConst(builder: PsiBuilder): Boolean {
        if (!builder.getTokenType().equals(NasoTypes.CONST)) return false
        
        val marker = builder.mark()
        builder.advanceLexer() // const
        
        if (builder.getTokenType().equals(NasoTypes.IDENTIFIER)) {
            builder.advanceLexer()
        } else {
            marker.error("Expected const name")
        }
        
        if (builder.getTokenType().toString() == ":") {
            builder.advanceLexer()
            parseType(builder)
        }
        
        if (builder.getTokenType().toString() == "=") {
            builder.advanceLexer()
            parseExpression(builder)
        }
        
        marker.done(NasoTypes.CONST_DEF)
        return true
    }
    
    private fun parseCircuit(builder: PsiBuilder): Boolean {
        if (!builder.getTokenType().equals(NasoTypes.CIRCUIT)) return false
        
        val marker = builder.mark()
        builder.advanceLexer() // circuit
        
        if (builder.getTokenType().equals(NasoTypes.IDENTIFIER)) {
            builder.advanceLexer()
        } else {
            marker.error("Expected circuit name")
        }
        
        if (builder.getTokenType().toString() == "{") {
            parseBlock(builder)
        } else {
            marker.error("Expected circuit body")
        }
        
        marker.done(NasoTypes.CIRCUIT_DEF)
        return true
    }
    
    private fun parseUse(builder: PsiBuilder): Boolean {
        if (!builder.getTokenType().equals(NasoTypes.USE)) return false
        
        val marker = builder.mark()
        builder.advanceLexer() // use
        
        // Parse use path
        while (!builder.eof() && builder.getTokenType().toString() != ";") {
            builder.advanceLexer()
        }
        
        if (builder.getTokenType().toString() == ";") {
            builder.advanceLexer()
        }
        
        marker.done(NasoTypes.USE)
        return true
    }
    
    private fun parseMod(builder: PsiBuilder): Boolean {
        if (!builder.getTokenType().equals(NasoTypes.MOD)) return false
        
        val marker = builder.mark()
        builder.advanceLexer() // mod
        
        if (builder.getTokenType().equals(NasoTypes.IDENTIFIER)) {
            builder.advanceLexer()
        }
        
        if (builder.getTokenType().toString() == ";") {
            builder.advanceLexer()
        } else if (builder.getTokenType().toString() == "{") {
            parseBlock(builder)
        }
        
        marker.done(NasoTypes.MOD)
        return true
    }
    
    private fun parseGenerics(builder: PsiBuilder) {
        builder.advanceLexer() // <
        while (!builder.eof() && builder.getTokenType().toString() != ">") {
            if (builder.getTokenType().equals(NasoTypes.IDENTIFIER)) {
                builder.advanceLexer()
            }
            if (builder.getTokenType().toString() == ",") {
                builder.advanceLexer()
            } else {
                break
            }
        }
        if (builder.getTokenType().toString() == ">") {
            builder.advanceLexer()
        }
    }
    
    private fun parseParameters(builder: PsiBuilder) {
        builder.advanceLexer() // (
        while (!builder.eof() && builder.getTokenType().toString() != ")") {
            if (builder.getTokenType().equals(NasoTypes.IDENTIFIER)) {
                builder.advanceLexer()
            }
            if (builder.getTokenType().toString() == ":") {
                builder.advanceLexer()
                parseType(builder)
            }
            if (builder.getTokenType().toString() == ",") {
                builder.advanceLexer()
            } else {
                break
            }
        }
        if (builder.getTokenType().toString() == ")") {
            builder.advanceLexer()
        }
    }
    
    private fun parseType(builder: PsiBuilder): Boolean {
        return when {
            builder.getTokenType().equals(NasoTypes.IDENTIFIER) -> {
                builder.advanceLexer()
                true
            }
            builder.getTokenType().toString() == "&" -> {
                builder.advanceLexer()
                if (builder.getTokenType().equals(NasoTypes.MUT)) {
                    builder.advanceLexer()
                }
                parseType(builder)
            }
            builder.getTokenType().toString() == "[" -> {
                builder.advanceLexer()
                parseType(builder)
                if (builder.getTokenType().toString() == ";") {
                    builder.advanceLexer()
                    // Array size
                    builder.advanceLexer()
                }
                if (builder.getTokenType().toString() == "]") {
                    builder.advanceLexer()
                }
                true
            }
            builder.getTokenType().toString() == "(" -> {
                builder.advanceLexer()
                while (!builder.eof() && builder.getTokenType().toString() != ")") {
                    parseType(builder)
                    if (builder.getTokenType().toString() == ",") {
                        builder.advanceLexer()
                    }
                }
                if (builder.getTokenType().toString() == ")") {
                    builder.advanceLexer()
                }
                if (builder.getTokenType().toString() == "->") {
                    builder.advanceLexer()
                    parseType(builder)
                }
                true
            }
            else -> false
        }
    }
    
    private fun parseWhereClause(builder: PsiBuilder) {
        builder.advanceLexer() // where
        while (!builder.eof() && builder.getTokenType().toString() != "{") {
            builder.advanceLexer()
        }
    }
    
    private fun parseBlock(builder: PsiBuilder) {
        builder.advanceLexer() // {
        while (!builder.eof() && builder.getTokenType().toString() != "}") {
            parseStatement(builder)
        }
        if (builder.getTokenType().toString() == "}") {
            builder.advanceLexer()
        }
    }
    
    private fun parseStatement(builder: PsiBuilder): Boolean {
        return when {
            parseLetBinding(builder) -> true
            parseConst(builder) -> true
            parseExpressionStatement(builder) -> true
            else -> {
                builder.advanceLexer()
                true
            }
        }
    }
    
    private fun parseExpressionStatement(builder: PsiBuilder): Boolean {
        val startToken = builder.getTokenType()
        parseExpression(builder)
        return true
    }
    
    private fun parseExpression(builder: PsiBuilder): Boolean {
        // Simplified expression parsing
        parsePrimaryExpression(builder)
        
        while (!builder.eof()) {
            val token = builder.getTokenType()
            if (isBinaryOperator(token)) {
                builder.advanceLexer()
                parsePrimaryExpression(builder)
            } else {
                break
            }
        }
        
        return true
    }
    
    private fun parsePrimaryExpression(builder: PsiBuilder): Boolean {
        return when {
            builder.getTokenType().equals(NasoTypes.INTEGER) ||
            builder.getTokenType().equals(NasoTypes.FLOAT) ||
            builder.getTokenType().equals(NasoTypes.STRING_LITERAL) ||
            builder.getTokenType().equals(NasoTypes.TRUE) ||
            builder.getTokenType().equals(NasoTypes.FALSE) -> {
                builder.advanceLexer()
                true
            }
            builder.getTokenType().equals(NasoTypes.IDENTIFIER) -> {
                builder.advanceLexer()
                // Could be function call, method call, field access
                if (builder.getTokenType().toString() == "(") {
                    parseArguments(builder)
                } else if (builder.getTokenType().toString() == ".") {
                    builder.advanceLexer()
                    if (builder.getTokenType().equals(NasoTypes.IDENTIFIER)) {
                        builder.advanceLexer()
                        if (builder.getTokenType().toString() == "(") {
                            parseArguments(builder)
                        }
                    }
                }
                true
            }
            builder.getTokenType().toString() == "(" -> {
                builder.advanceLexer()
                parseExpression(builder)
                if (builder.getTokenType().toString() == ")") {
                    builder.advanceLexer()
                }
                true
            }
            builder.getTokenType().equals(NasoTypes.IF) -> parseIfExpression(builder)
            builder.getTokenType().equals(NasoTypes.MATCH) -> parseMatchExpression(builder)
            builder.getTokenType().equals(NasoTypes.LOOP) -> parseLoopExpression(builder)
            builder.getTokenType().equals(NasoTypes.WHILE) -> parseWhileExpression(builder)
            builder.getTokenType().equals(NasoTypes.FOR) -> parseForExpression(builder)
            builder.getTokenType().toString() == "{" -> { parseBlock(builder); true }
            else -> false
        }
    }
    
    private fun parseArguments(builder: PsiBuilder) {
        builder.advanceLexer() // (
        while (!builder.eof() && builder.getTokenType().toString() != ")") {
            parseExpression(builder)
            if (builder.getTokenType().toString() == ",") {
                builder.advanceLexer()
            }
        }
        if (builder.getTokenType().toString() == ")") {
            builder.advanceLexer()
        }
    }
    
    private fun parseIfExpression(builder: PsiBuilder): Boolean {
        val marker = builder.mark()
        builder.advanceLexer() // if
        parseExpression(builder)
        if (builder.getTokenType().toString() == "{") {
            parseBlock(builder)
        }
        if (builder.getTokenType().equals(NasoTypes.ELSE)) {
            builder.advanceLexer()
            if (builder.getTokenType().equals(NasoTypes.IF)) {
                parseIfExpression(builder)
            } else if (builder.getTokenType().toString() == "{") {
                parseBlock(builder)
            }
        }
        marker.done(NasoTypes.IF_EXPR)
        return true
    }
    
    private fun parseMatchExpression(builder: PsiBuilder): Boolean {
        val marker = builder.mark()
        builder.advanceLexer() // match
        parseExpression(builder)
        if (builder.getTokenType().toString() == "{") {
            parseBlock(builder)
        }
        marker.done(NasoTypes.MATCH_EXPR)
        return true
    }
    
    private fun parseLoopExpression(builder: PsiBuilder): Boolean {
        val marker = builder.mark()
        builder.advanceLexer() // loop
        if (builder.getTokenType().toString() == "{") {
            parseBlock(builder)
        }
        marker.done(NasoTypes.LOOP_EXPR)
        return true
    }
    
    private fun parseWhileExpression(builder: PsiBuilder): Boolean {
        val marker = builder.mark()
        builder.advanceLexer() // while
        parseExpression(builder)
        if (builder.getTokenType().toString() == "{") {
            parseBlock(builder)
        }
        marker.done(NasoTypes.WHILE_EXPR)
        return true
    }
    
    private fun parseForExpression(builder: PsiBuilder): Boolean {
        val marker = builder.mark()
        builder.advanceLexer() // for
        if (builder.getTokenType().equals(NasoTypes.IDENTIFIER)) {
            builder.advanceLexer()
        }
        if (builder.getTokenType().toString() == "in") {
            builder.advanceLexer()
        }
        parseExpression(builder)
        if (builder.getTokenType().toString() == "{") {
            parseBlock(builder)
        }
        marker.done(NasoTypes.FOR_EXPR)
        return true
    }
    
    private fun isBinaryOperator(token: IElementType?): Boolean {
        val text = token?.toString() ?: ""
        return text in setOf("+", "-", "*", "/", "%", "**",
            "==", "!=", "<", ">", "<=", ">=",
            "&&", "||",
            "=", "+=", "-=", "*=", "/=", "%=")
    }
}