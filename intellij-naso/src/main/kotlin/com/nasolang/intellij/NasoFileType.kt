package com.nasolang.intellij

import com.intellij.lang.Language

object NasoLanguage : Language("Naso")

class NasoFileType : com.intellij.openapi.fileTypes.LanguageFileType(NasoLanguage) {
    companion object {
        val INSTANCE = NasoFileType()
    }

    override fun getName(): String = "Naso"
    override fun getDescription(): String = "Naso source file"
    override fun getDefaultExtension(): String = "naso"
    override fun getIcon(): javax.swing.Icon = com.intellij.openapi.util.IconLoader.getIcon("/icons/naso_file.svg", javaClass.classLoader)
}