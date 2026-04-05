/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{js,ts,jsx,tsx}"],
  theme: {
    extend: {
      colors: {
        tm: {
          bg: "#0a0a0f",
          surface: "#12121a",
          border: "#1e1e2e",
          accent: "#7c5cfc",
          accent2: "#5ca0fc",
          text: "#e0e0e8",
          muted: "#6b6b80",
          green: "#4ade80",
          red: "#f87171",
          yellow: "#fbbf24",
        },
      },
    },
  },
  plugins: [],
};
