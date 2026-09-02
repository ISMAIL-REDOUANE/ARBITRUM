/** @type {import('tailwindcss').Config} */
export default {
  content: [
    "./index.html",
    "./src/**/*.{js,ts,jsx,tsx}",
  ],
  darkMode: 'class',
  theme: {
    extend: {
      colors: {
        cyber: {
          black: '#0d0e12',
          dark: '#12141a',
          panel: '#1a1d25',
          border: '#2a2e38',
          muted: '#4a4e58',
          text: '#e0e2e8',
          green: '#00ff9d',
          'green-dim': '#00cc7d',
          cyan: '#00d4ff',
          orange: '#ff9d00',
          red: '#ff4757',
          'red-dim': '#cc3a47',
        }
      },
      fontFamily: {
        mono: ['JetBrains Mono', 'Fira Code', 'Consolas', 'monospace'],
      },
      animation: {
        'pulse-fast': 'pulse 1s cubic-bezier(0.4, 0, 0.6, 1) infinite',
        'glow': 'glow 2s ease-in-out infinite alternate',
      },
      keyframes: {
        glow: {
          '0%': { boxShadow: '0 0 5px #00ff9d, 0 0 10px #00ff9d' },
          '100%': { boxShadow: '0 0 10px #00ff9d, 0 0 20px #00ff9d, 0 0 30px #00ff9d' },
        }
      }
    },
  },
  plugins: [],
}
