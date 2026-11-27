package com.nasolang.intellij

import com.intellij.lang.Commenter

class NasoCommenter : Commenter {
    override fun getLineCommentPrefix(): String = "//"
    
    override fun getBlockCommentPrefix(): String = "/*"
    
    override fun getBlockCommentSuffix(): String = "*/"
    
    override fun getCommentedBlockCommentPrefix(): String = "/*"
    
    override fun getCommentedBlockCommentSuffix(): String = "*/"
}