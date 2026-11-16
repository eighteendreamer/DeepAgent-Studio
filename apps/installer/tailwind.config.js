export default {
  content: ["./index.html", "./src/**/*.{ts,tsx}", "../desktop/src/components/shadcn/{button,input,label,checkbox,progress}.tsx"],
  theme: { extend: { colors: {
    primary: "#111827", "primary-hover": "#000000", "text-base": "#1F2937",
    "text-secondary": "#6B7280", "text-tertiary": "#9CA3AF", "border-theme": "#E5E7EB",
  } } }, plugins: [],
};
