import { defineConfig } from 'vite';
import wasm from 'vite-plugin-wasm';
import path from 'path';

// Custom plugin to rewrite wasm-pack output
function wasmPackRewrite() {
  return {
    name: 'wasm-pack-rewrite',
    enforce: 'post',
    transform(code, id) {
      if (id.includes('nasoc_wasm.js') && !id.includes('node_modules')) {
        // Replace hardcoded wasm filename with Vite-resolved URL
        return code.replace(
          /new URL\('nasoc_wasm_bg\.wasm', import\.meta\.url\)/g,
          '__NASOC_WASM_URL__'
        );
      }
      return null;
    },
    generateBundle(options, bundle) {
      // Find the wasm file and the nasoc_wasm.js chunk
      const wasmFile = Object.values(bundle).find(f => f.type === 'asset' && f.name?.endsWith('.wasm'));
      const nasocWasmChunk = Object.values(bundle).find(f => f.type === 'chunk' && f.name?.includes('nasoc_wasm'));
      
      if (wasmFile && nasocWasmChunk) {
        const wasmUrl = wasmFile.fileName;
        // Replace the placeholder with the actual wasm URL
        nasocWasmChunk.code = nasocWasmChunk.code.replace(
          /__NASOC_WASM_URL__/g,
          JSON.stringify(`/${wasmUrl}`)
        );
      }
    },
  };
}

export default defineConfig({
  base: process.env.BASE_URL || '/',
  plugins: [
    wasm(),
    wasmPackRewrite(),
  ],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  build: {
    outDir: 'dist',
    target: 'es2022',
    rollupOptions: {
      output: {
        manualChunks: {
          monaco: ['monaco-editor'],
        },
      },
    },
  },
  server: {
    port: 3000,
    open: true,
    headers: {
      'Cache-Control': 'no-store, no-cache, must-revalidate, proxy-revalidate',
      'Pragma': 'no-cache',
      'Expires': '0',
    },
  },
  preview: {
    port: 4173,
    headers: {
      'Cache-Control': 'no-store, no-cache, must-revalidate, proxy-revalidate',
      'Pragma': 'no-cache',
      'Expires': '0',
    },
  },
  worker: {
    format: 'es',
  },
  optimizeDeps: {
    exclude: ['@nasoc-wasm'],
  },
});