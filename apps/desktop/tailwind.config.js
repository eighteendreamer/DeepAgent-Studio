/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        primary: "#111827",
        "primary-hover": "#000000",
        "text-base": "#1F2937",
        "text-secondary": "#6B7280",
        "border-theme": "#E5E7EB",
        "sidebar-bg": "#F9F8F6", // warm light-gray sidebar
      },
      fontFamily: {
        sans: [
          "Inter",
          "-apple-system",
          "BlinkMacSystemFont",
          '"Segoe UI"',
          "Roboto",
          '"Helvetica Neue"',
          "Arial",
          "sans-serif",
        ],
      },
    },
  },
  plugins: [],
};
