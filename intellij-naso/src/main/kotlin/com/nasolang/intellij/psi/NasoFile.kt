package com.nasolang.intellij.psi

import com.intellij.extapi.psi.PsiFileBase
import com.intellij.openapi.fileTypes.FileType
import com.intellij.psi.FileViewProvider
import com.nasolang.intellij.NasoLanguage
import org.jetbrains.annotations.NotNull

class NasoFile(viewProvider: FileViewProvider) : PsiFileBase(viewProvider, NasoLanguage.INSTANCE) {
    override fun getFileType(): FileType = NasoFileType.INSTANCE
    
    override fun toString(): String = "NasoFile:${name}"
}