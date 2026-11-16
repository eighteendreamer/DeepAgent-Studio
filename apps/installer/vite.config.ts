import { defineConfig } from "vite";
import react from "@vitejs/plugin-react";
export default defineConfig({
  plugins: [react()], base: "./", clearScreen: false,
  resolve: { dedupe: ["react", "react-dom", "gsap", "radix-ui", "clsx", "tailwind-merge", "lucide-react"] },
  server: { port: 1421, strictPort: true, fs: { allow: [".."] } },
  build: { target: "es2020" },
});
