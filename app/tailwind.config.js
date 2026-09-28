/** @type {import('tailwindcss').Config} */
export default {
  content: [
    './components/**/*.{js,vue,ts}',
    './layouts/**/*.{js,vue,ts}',
    './pages/**/*.{js,vue,ts}',
    './plugins/**/*.{js,vue,ts}',
    './app.vue',
    './error.vue',
  ],
  darkMode: 'class',
  theme: {
    extend: {
      colors: {
        // Canvas Background
        'canvas': '#050505',
        // Panel / Surface
        'surface': '#0F0F11',
        // Subtle Border
        'border-subtle': '#27272A',
        // Primary Text
        'text-primary': '#EDEDEF',
        // Body / Muted Text
        'text-muted': '#A1A1AA',
        // Dimmed Text
        'text-dimmed': '#52525B',
        // Accent Action / Active Status
        'accent': '#10B981',
        // Warning / VRAM Pressure
        'warning': '#F59E0B',
        // Error
        'error': '#EF4444',
      },
      fontFamily: {
        sans: ['Geist', 'Inter', '-apple-system', 'sans-serif'],
        mono: ['Geist Mono', 'JetBrains Mono', 'Fira Code', 'monospace'],
      },
      fontSize: {
        'body': ['14px', { lineHeight: '1.5', letterSpacing: '-0.01em' }],
      },
      borderWidth: {
        'hairline': '1px',
      },
      borderRadius: {
        'palette': '8px',
      },
      spacing: {
        '32': '32px',
      },
      height: {
        'telemetry': '32px',
      },
    },
  },
  plugins: [],
}