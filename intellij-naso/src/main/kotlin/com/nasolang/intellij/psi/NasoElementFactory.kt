package com.nasolang.intellij.psi

import com.intellij.lang.ASTNode
import com.intellij.psi.PsiElement

object NasoElementFactory {
    fun createElement(node: ASTNode): PsiElement {
        return when (node.elementType) {
            NasoTypes.FUNCTION_DEF -> NasoFunctionDefinition(node)
            NasoTypes.STRUCT_DEF -> NasoStructDefinition(node)
            NasoTypes.ENUM_DEF -> NasoEnumDefinition(node)
            NasoTypes.IMPL_BLOCK -> NasoImplBlock(node)
            NasoTypes.TRAIT_DEF -> NasoTraitDefinition(node)
            NasoTypes.TYPE_ALIAS -> NasoTypeAlias(node)
            NasoTypes.LET_BINDING -> NasoLetBinding(node)
            NasoTypes.CONST_DEF -> NasoConstDefinition(node)
            NasoTypes.CIRCUIT_DEF -> NasoCircuitDefinition(node)
            else -> NasoCompositeElement(node)
        }
    }
}

abstract class NasoCompositeElement(node: ASTNode) : com.intellij.extapi.psi.ASTWrapperPsiElement(node)

class NasoFunctionDefinition(node: ASTNode) : NasoCompositeElement(node) {
    fun getName(): String? {
        val nameNode = node.findChildByType(NasoTypes.IDENTIFIER)
        return nameNode?.text
    }
    
    fun getReturnType(): String? {
        val typeNode = node.findChildByType(NasoTypes.FUNCTION_TYPE)
        return typeNode?.text
    }
}

class NasoStructDefinition(node: ASTNode) : NasoCompositeElement(node) {
    fun getName(): String? {
        val nameNode = node.findChildByType(NasoTypes.IDENTIFIER)
        return nameNode?.text
    }
    
    fun getFields(): List<NasoField> {
        val fields = mutableListOf<NasoField>()
        val children = node.children
        for (child in children) {
            if (child.elementType == NasoTypes.FIELD_DEF) {
                fields.add(NasoField(child))
            }
        }
        return fields
    }
}

class NasoField(node: ASTNode) : NasoCompositeElement(node) {
    fun getName(): String? {
        val nameNode = node.findChildByType(NasoTypes.IDENTIFIER)
        return nameNode?.text
    }
    
    fun getType(): String? {
        val typeNode = node.findChildByType(NasoTypes.PRIMITIVE_TYPE)
            ?: node.findChildByType(NasoTypes.GENERIC_TYPE)
            ?: node.findChildByType(NasoTypes.REFERENCE_TYPE)
        return typeNode?.text
    }
}

class NasoEnumDefinition(node: ASTNode) : NasoCompositeElement(node) {
    fun getName(): String? {
        val nameNode = node.findChildByType(NasoTypes.IDENTIFIER)
        return nameNode?.text
    }
    
    fun getVariants(): List<NasoEnumVariant> {
        val variants = mutableListOf<NasoEnumVariant>()
        val children = node.children
        for (child in children) {
            if (child.elementType == NasoTypes.ENUM_VARIANT) {
                variants.add(NasoEnumVariant(child))
            }
        }
        return variants
    }
}

class NasoEnumVariant(node: ASTNode) : NasoCompositeElement(node) {
    fun getName(): String? {
        val nameNode = node.findChildByType(NasoTypes.IDENTIFIER)
        return nameNode?.text
    }
}

class NasoImplBlock(node: ASTNode) : NasoCompositeElement(node) {
    fun getTraitName(): String? {
        // Implementation would parse the trait name from the impl block
        return null
    }
    
    fun getTypeName(): String? {
        // Implementation would parse the type name
        return null
    }
}

class NasoTraitDefinition(node: ASTNode) : NasoCompositeElement(node) {
    fun getName(): String? {
        val nameNode = node.findChildByType(NasoTypes.IDENTIFIER)
        return nameNode?.text
    }
}

class NasoTypeAlias(node: ASTNode) : NasoCompositeElement(node) {
    fun getName(): String? {
        val nameNode = node.findChildByType(NasoTypes.IDENTIFIER)
        return nameNode?.text
    }
    
    fun getTargetType(): String? {
        // Implementation would parse the target type
        return null
    }
}

class NasoLetBinding(node: ASTNode) : NasoCompositeElement(node) {
    fun getName(): String? {
        val nameNode = node.findChildByType(NasoTypes.IDENTIFIER)
        return nameNode?.text
    }
    
    fun isMutable(): Boolean {
        return node.findChildByType(NasoTypes.MUT) != null
    }
}

class NasoConstDefinition(node: ASTNode) : NasoCompositeElement(node) {
    fun getName(): String? {
        val nameNode = node.findChildByType(NasoTypes.IDENTIFIER)
        return nameNode?.text
    }
}

class NasoCircuitDefinition(node: ASTNode) : NasoCompositeElement(node) {
    fun getName(): String? {
        val nameNode = node.findChildByType(NasoTypes.IDENTIFIER)
        return nameNode?.text
    }
    
    fun getQubitCount(): Int {
        val qubitsNode = node.findChildByType(NasoTypes.QUBITS)
        return qubitsNode?.text?.toInt() ?: 0
    }
    
    fun getGates(): List<NasoGateApplication> {
        val gates = mutableListOf<NasoGateApplication>()
        val children = node.children
        for (child in children) {
            if (child.elementType == NasoTypes.GATE_APPLICATION) {
                gates.add(NasoGateApplication(child))
            }
        }
        return gates
    }
}

class NasoGateApplication(node: ASTNode) : NasoCompositeElement(node) {
    fun getGateName(): String? {
        val gateNode = node.findChildByType(NasoTypes.GATE)
            ?: node.findChildByType(NasoTypes.H)
            ?: node.findChildByType(NasoTypes.X)
            ?: node.findChildByType(NasoTypes.Y)
            ?: node.findChildByType(NasoTypes.Z)
            ?: node.findChildByType(NasoTypes.CNOT)
            ?: node.findChildByType(NasoTypes.CZ)
            ?: node.findChildByType(NasoTypes.RX)
            ?: node.findChildByType(NasoTypes.RY)
            ?: node.findChildByType(NasoTypes.RZ)
            ?: node.findChildByType(NasoTypes.S)
            ?: node.findChildByType(NasoTypes.T)
        return gateNode?.text
    }
    
    fun getQubits(): List<Int> {
        val qubits = mutableListOf<Int>()
        val children = node.children
        for (child in children) {
            if (child.elementType == NasoTypes.INTEGER) {
                qubits.add(child.text.toInt())
            }
        }
        return qubits
    }
}