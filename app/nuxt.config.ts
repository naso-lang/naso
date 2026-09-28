import tailwindcss from '@nuxtjs/tailwindcss'
import vitePwa from '@vite-pwa/nuxt'

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
      ],
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