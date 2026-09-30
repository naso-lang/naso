import tailwindcss from '@nuxtjs/tailwindcss'
import vitePwa from '@vite-pwa/nuxt'

// Where the generated site will be served from. GitHub Pages mounts this repo
// at /naso/, Netlify and local dev serve from the root. Trailing slash matters
// for Nuxt's path joining, so it is normalised here rather than at each use.
const baseURL = process.env.NUXT_APP_BASE_URL || '/'

export default defineNuxtConfig({
  ssr: false,
  devtools: { enabled: true },

  modules: [
    '@nuxtjs/tailwindcss',
    '@vite-pwa/nuxt',
  ],
  
  css: ['~/assets/css/main.css'],
  
  tailwindcss: {
    configPath: 'tailwind.config.js',
    exposeConfig: true,
  },
  
  pwa: {
    registerType: 'autoUpdate',
    manifest: {
      name: 'NasoChat',
      short_name: 'NasoChat',
      description: 'High-performance WebGPU-powered chat interface',
      theme_color: '#050505',
      background_color: '#050505',
      display: 'standalone',
      icons: [
        {
          src: '/icon-192.svg',
          sizes: '192x192',
          type: 'image/svg+xml',
          purpose: 'any maskable',
        },
        {
          src: '/icon-512.svg',
          sizes: '512x512',
          type: 'image/svg+xml',
          purpose: 'any maskable',
        },
      ],
    },
    workbox: {
      globPatterns: ['**/*.{js,css,html,ico,png,svg,woff2}'],
      maximumFileSizeToCacheInBytes: 5 * 1024 * 1024,
    },
    devOptions: {
      enabled: true,
      type: 'module',
    },
  },
  
  app: {
    // GitHub Pages serves this repo at /naso/ (a subpath), while Netlify and
    // local dev serve it at the domain root. Nuxt derives asset and router
    // paths from this, so it must match where the site is actually served or
    // every asset 404s. The deploy workflow sets NUXT_APP_BASE_URL; it
    // defaults to "/" so local dev and the Netlify build are unaffected.
    baseURL,
    head: {
      title: 'NasoChat',
      meta: [
        { name: 'viewport', content: 'width=device-width, initial-scale=1' },
        { name: 'theme-color', content: '#050505' },
      ],
      link: [
        { rel: 'preconnect', href: 'https://fonts.googleapis.com' },
        { rel: 'preconnect', href: 'https://fonts.gstatic.com', crossorigin: '' },
        { rel: 'stylesheet', href: 'https://fonts.googleapis.com/css2?family=Geist:wght@400;500&family=Geist+Mono:wght@400;500&display=swap' },
        // COOP/COEP for SharedArrayBuffer.
        // These must be built from baseURL. Nuxt emits app.head script/link
        // URLs verbatim, so a bare "/coi-serviceworker.js" stays rooted at the
        // domain and 404s under a subpath deploy like GitHub Pages (/naso/).
        { rel: 'modulepreload', href: `${baseURL}coi-serviceworker.js` },
      ],
      script: [
        { src: `${baseURL}coi-serviceworker.js`, type: 'module', crossorigin: 'anonymous' },
      ],
    },
  },
  
  nitro: {
    preset: 'static',
    static: {
      generate: true,
    },
    routeRules: {
      '/**': {
        headers: {
          'Cross-Origin-Opener-Policy': 'same-origin',
          'Cross-Origin-Embedder-Policy': 'require-corp',
        },
      },
    },
  },
  
  vite: {
    optimizeDeps: {
      include: ['vue', '@vueuse/core'],
    },
    build: {
      target: 'esnext',
    },
  },
  
  compatibilityDate: '2024-09-28',
})