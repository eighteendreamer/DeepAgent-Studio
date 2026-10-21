import tailwindcss from "tailwindcss";
import autoprefixer from "autoprefixer";
import continuousCorners from "./scripts/continuous-corners.postcss.mjs";

export default {
  plugins: [tailwindcss(), continuousCorners(), autoprefixer()],
};
