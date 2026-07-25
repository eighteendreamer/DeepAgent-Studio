/** @type {import('tailwindcss').Config} */
export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}"],
  theme: {
    extend: {
      colors: {
        primary: "var(--theme-accent, #111827)",
        "primary-hover": "var(--theme-accent-hover, #000000)",
        "bg-base": "var(--theme-bg, #ffffff)",
        "text-base": "var(--theme-fg, #1F2937)",
        "text-secondary": "var(--theme-text-secondary, #6B7280)",
        "border-theme": "var(--theme-border, #E5E7EB)",
        "sidebar-bg": "var(--theme-sidebar, #F9F8F6)",
        "elevated-bg": "var(--theme-elevated, #FFFFFF)",
        "hover-bg": "var(--theme-hover, #F3F4F6)",
        "selection-bg": "var(--theme-selection, #DBEAFE)",
      },
      fontFamily: {
        sans: [
          "var(--ui-font, Inter)",
          "-apple-system",
          "BlinkMacSystemFont",
          '"Segoe UI"',
          "Roboto",
          '"Helvetica Neue"',
          "Arial",
          "sans-serif",
        ],
        mono: [
          "var(--code-font, ui-monospace)",
          "SFMono-Regular",
          "Menlo",
          "Monaco",
          "Consolas",
          '"Liberation Mono"',
          '"Courier New"',
          "monospace",
        ],
      },
    },
  },
  plugins: [],
};
