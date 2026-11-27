package com.nasolang.intellij

import com.intellij.navigation.ChooseByNameContributor
import com.intellij.openapi.project.Project
import com.intellij.psi.PsiElement
import com.intellij.psi.search.GlobalSearchScope
import com.nasolang.intellij.psi.NasoFunctionDefinition
import com.nasolang.intellij.psi.NasoStubIndex
import org.jetbrains.annotations.NotNull

class NasoGotoSymbolContributor : ChooseByNameContributor {
    @NotNull
    override fun getNames(@NotNull project: Project, @NotNull includeNonProjectItems: Boolean): Array<String> {
        val names = mutableSetOf<String>()
        val functions = NasoStubIndex.KEY.getAllKeys(project)
        names.addAll(functions)
        return names.toTypedArray()
    }

    @NotNull
    override fun getElementsByName(
        @NotNull name: String,
        @NotNull project: Project,
        @NotNull includeNonProjectItems: Boolean
    ): Array<PsiElement> {
        val scope = if (includeNonProjectItems) GlobalSearchScope.allScope(project) else GlobalSearchScope.projectScope(project)
        val functions = NasoStubIndex.KEY.get(name, project, scope)
        return functions.toTypedArray()
    }
}