package com.nasolang.intellij.psi

import com.intellij.psi.stubs.StubIndexKey
import com.intellij.psi.stubs.StringStubIndexExtension
import com.nasolang.intellij.psi.NasoTypes

class NasoStubIndex : StringStubIndexExtension<NasoFunctionDefinition>() {
    override fun getKey(): StubIndexKey<String, NasoFunctionDefinition> = NasoStubIndex.KEY
    
    override fun getVersion(): Int = 1
    
    companion object {
        val KEY = StubIndexKey.create<String, NasoFunctionDefinition>("naso.functions", NasoFunctionDefinition::class.java)
    }
}