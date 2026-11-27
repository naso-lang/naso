package com.nasolang.intellij.psi

import com.intellij.psi.tree.IElementType
import com.intellij.psi.TokenType

object NasoTypes {
    // Keywords
    val FN = IElementType("FN", NasoLanguage.INSTANCE)
    val LET = IElementType("LET", NasoLanguage.INSTANCE)
    val MUT = IElementType("MUT", NasoLanguage.INSTANCE)
    val CONST = IElementType("CONST", NasoLanguage.INSTANCE)
    val STRUCT = IElementType("STRUCT", NasoLanguage.INSTANCE)
    val ENUM = IElementType("ENUM", NasoLanguage.INSTANCE)
    val IMPL = IElementType("IMPL", NasoLanguage.INSTANCE)
    val TRAIT = IElementType("TRAIT", NasoLanguage.INSTANCE)
    val TYPE = IElementType("TYPE", NasoLanguage.INSTANCE)
    val IF = IElementType("IF", NasoLanguage.INSTANCE)
    val ELSE = IElementType("ELSE", NasoLanguage.INSTANCE)
    val MATCH = IElementType("MATCH", NasoLanguage.INSTANCE)
    val LOOP = IElementType("LOOP", NasoLanguage.INSTANCE)
    val WHILE = IElementType("WHILE", NasoLanguage.INSTANCE)
    val FOR = IElementType("FOR", NasoLanguage.INSTANCE)
    val BREAK = IElementType("BREAK", NasoLanguage.INSTANCE)
    val CONTINUE = IElementType("CONTINUE", NasoLanguage.INSTANCE)
    val RETURN = IElementType("RETURN", NasoLanguage.INSTANCE)
    val PUB = IElementType("PUB", NasoLanguage.INSTANCE)
    val USE = IElementType("USE", NasoLanguage.INSTANCE)
    val MOD = IElementType("MOD", NasoLanguage.INSTANCE)
    val AS = IElementType("AS", NasoLanguage.INSTANCE)
    val WHERE = IElementType("WHERE", NasoLanguage.INSTANCE)
    val ASYNC = IElementType("ASYNC", NasoLanguage.INSTANCE)
    val AWAIT = IElementType("AWAIT", NasoLanguage.INSTANCE)
    val MOVE = IElementType("MOVE", NasoLanguage.INSTANCE)
    
    // Quantum keywords
    val CIRCUIT = IElementType("CIRCUIT", NasoLanguage.INSTANCE)
    val QUBITS = IElementType("QUBITS", NasoLanguage.INSTANCE)
    val GATE = IElementType("GATE", NasoLanguage.INSTANCE)
    val MEASURE = IElementType("MEASURE", NasoLanguage.INSTANCE)
    val QALLOC = IElementType("QALLOC", NasoLanguage.INSTANCE)
    val QFREE = IElementType("QFREE", NasoLanguage.INSTANCE)
    
    // Quantum gates
    val H = IElementType("H", NasoLanguage.INSTANCE)
    val X = IElementType("X", NasoLanguage.INSTANCE)
    val Y = IElementType("Y", NasoLanguage.INSTANCE)
    val Z = IElementType("Z", NasoLanguage.INSTANCE)
    val CNOT = IElementType("CNOT", NasoLanguage.INSTANCE)
    val CZ = IElementType("CZ", NasoLanguage.INSTANCE)
    val RX = IElementType("RX", NasoLanguage.INSTANCE)
    val RY = IElementType("RY", NasoLanguage.INSTANCE)
    val RZ = IElementType("RZ", NasoLanguage.INSTANCE)
    val S = IElementType("S", NasoLanguage.INSTANCE)
    val T = IElementType("T", NasoLanguage.INSTANCE)
    
    // Literals
    val TRUE = IElementType("TRUE", NasoLanguage.INSTANCE)
    val FALSE = IElementType("FALSE", NasoLanguage.INSTANCE)
    val NONE = IElementType("NONE", NasoLanguage.INSTANCE)
    val SOME = IElementType("SOME", NasoLanguage.INSTANCE)
    
    // Composite types
    val FUNCTION_DEF = IElementType("FUNCTION_DEF", NasoLanguage.INSTANCE)
    val STRUCT_DEF = IElementType("STRUCT_DEF", NasoLanguage.INSTANCE)
    val ENUM_DEF = IElementType("ENUM_DEF", NasoLanguage.INSTANCE)
    val IMPL_BLOCK = IElementType("IMPL_BLOCK", NasoLanguage.INSTANCE)
    val TRAIT_DEF = IElementType("TRAIT_DEF", NasoLanguage.INSTANCE)
    val TYPE_ALIAS = IElementType("TYPE_ALIAS", NasoLanguage.INSTANCE)
    val LET_BINDING = IElementType("LET_BINDING", NasoLanguage.INSTANCE)
    val CONST_DEF = IElementType("CONST_DEF", NasoLanguage.INSTANCE)
    
    // Expressions
    val BINARY_EXPR = IElementType("BINARY_EXPR", NasoLanguage.INSTANCE)
    val UNARY_EXPR = IElementType("UNARY_EXPR", NasoLanguage.INSTANCE)
    val CALL_EXPR = IElementType("CALL_EXPR", NasoLanguage.INSTANCE)
    val METHOD_CALL = IElementType("METHOD_CALL", NasoLanguage.INSTANCE)
    val INDEX_EXPR = IElementType("INDEX_EXPR", NasoLanguage.INSTANCE)
    val FIELD_ACCESS = IElementType("FIELD_ACCESS", NasoLanguage.INSTANCE)
    val IF_EXPR = IElementType("IF_EXPR", NasoLanguage.INSTANCE)
    val MATCH_EXPR = IElementType("MATCH_EXPR", NasoLanguage.INSTANCE)
    val LOOP_EXPR = IElementType("LOOP_EXPR", NasoLanguage.INSTANCE)
    val WHILE_EXPR = IElementType("WHILE_EXPR", NasoLanguage.INSTANCE)
    val FOR_EXPR = IElementType("FOR_EXPR", NasoLanguage.INSTANCE)
    val BLOCK_EXPR = IElementType("BLOCK_EXPR", NasoLanguage.INSTANCE)
    val RETURN_EXPR = IElementType("RETURN_EXPR", NasoLanguage.INSTANCE)
    val BREAK_EXPR = IElementType("BREAK_EXPR", NasoLanguage.INSTANCE)
    val CONTINUE_EXPR = IElementType("CONTINUE_EXPR", NasoLanguage.INSTANCE)
    
    // Quantum expressions
    val CIRCUIT_DEF = IElementType("CIRCUIT_DEF", NasoLanguage.INSTANCE)
    val GATE_APPLICATION = IElementType("GATE_APPLICATION", NasoLanguage.INSTANCE)
    val MEASURE_EXPR = IElementType("MEASURE_EXPR", NasoLanguage.INSTANCE)
    val QALLOC_EXPR = IElementType("QALLOC_EXPR", NasoLanguage.INSTANCE)
    val QFREE_EXPR = IElementType("QFREE_EXPR", NasoLanguage.INSTANCE)
    
    // Types
    val PRIMITIVE_TYPE = IElementType("PRIMITIVE_TYPE", NasoLanguage.INSTANCE)
    val REFERENCE_TYPE = IElementType("REFERENCE_TYPE", NasoLanguage.INSTANCE)
    val MUTABLE_REFERENCE_TYPE = IElementType("MUTABLE_REFERENCE_TYPE", NasoLanguage.INSTANCE)
    val ARRAY_TYPE = IElementType("ARRAY_TYPE", NasoLanguage.INSTANCE)
    val TUPLE_TYPE = IElementType("TUPLE_TYPE", NasoLanguage.INSTANCE)
    val FUNCTION_TYPE = IElementType("FUNCTION_TYPE", NasoLanguage.INSTANCE)
    val GENERIC_TYPE = IElementType("GENERIC_TYPE", NasoLanguage.INSTANCE)
    val QUANTUM_TYPE = IElementType("QUANTUM_TYPE", NasoLanguage.INSTANCE)
    
    // Patterns
    val WILDCARD_PATTERN = IElementType("WILDCARD_PATTERN", NasoLanguage.INSTANCE)
    val IDENTIFIER_PATTERN = IElementType("IDENTIFIER_PATTERN", NasoLanguage.INSTANCE)
    val LITERAL_PATTERN = IElementType("LITERAL_PATTERN", NasoLanguage.INSTANCE)
    val TUPLE_PATTERN = IElementType("TUPLE_PATTERN", NasoLanguage.INSTANCE)
    val STRUCT_PATTERN = IElementType("STRUCT_PATTERN", NasoLanguage.INSTANCE)
    val ENUM_PATTERN = IElementType("ENUM_PATTERN", NasoLanguage.INSTANCE)
    val RANGE_PATTERN = IElementType("RANGE_PATTERN", NasoLanguage.INSTANCE)
    val OR_PATTERN = IElementType("OR_PATTERN", NasoLanguage.INSTANCE)
}