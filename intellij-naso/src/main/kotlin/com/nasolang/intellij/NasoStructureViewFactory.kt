package com.nasolang.intellij

import com.intellij.ide.structureView.*
import com.intellij.ide.util.treeView.smartTree.*
import com.intellij.psi.PsiElement
import com.nasolang.intellij.psi.*
import org.jetbrains.annotations.NotNull

class NasoStructureViewFactory : StructureViewBuilderFactory {
    @NotNull
    override fun getStructureViewBuilder(@NotNull psiFile: PsiFile): StructureViewBuilder {
        return NasoStructureViewBuilder(psiFile)
    }
}

class NasoStructureViewBuilder(private val psiFile: PsiFile) : StructureViewBuilder {
    @NotNull
    override fun getRootElement(): StructureViewTreeElement {
        return NasoStructureViewElement(psiFile)
    }
}

class NasoStructureViewElement(private val element: PsiElement) : StructureViewTreeElement, TreeElement {
    override fun getPresentation(): Presentation {
        return Presentation(element.getName() ?: element.getContainingFile().name)
    }

    @NotNull
    override fun getChildren(): Array<TreeElement> {
        val children = mutableListOf<TreeElement>()
        
        if (element is NasoFile) {
            for (child in element.children) {
                when (child) {
                    is NasoFunctionDefinition -> children.add(NasoStructureViewElement(child))
                    is NasoStructDefinition -> children.add(NasoStructureViewElement(child))
                    is NasoEnumDefinition -> children.add(NasoStructureViewElement(child))
                    is NasoImplBlock -> children.add(NasoStructureViewElement(child))
                    is NasoTraitDefinition -> children.add(NasoStructureViewElement(child))
                    is NasoTypeAlias -> children.add(NasoStructureViewElement(child))
                    is NasoCircuitDefinition -> children.add(NasoStructureViewElement(child))
                }
            }
        } else if (element is NasoStructDefinition) {
            for (field in element.getFields()) {
                children.add(NasoStructureViewElement(field))
            }
        } else if (element is NasoEnumDefinition) {
            for (variant in element.getVariants()) {
                children.add(NasoStructureViewElement(variant))
            }
        } else if (element is NasoCircuitDefinition) {
            for (gate in element.getGates()) {
                children.add(NasoStructureViewElement(gate))
            }
        }
        
        return children.toTypedArray()
    }

    override fun getParentElement(): TreeElement? = null

    override fun getValue(): Any = element
}