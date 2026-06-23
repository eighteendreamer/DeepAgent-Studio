import DefaultTheme from "vitepress/theme";
import HomePage from "./components/HomePage.vue";
import MermaidDiagram from "./components/MermaidDiagram.vue";
import { watch } from "vue";
import { useRoute } from "vitepress";
import "./styles.css";

export default {
  extends: DefaultTheme,
  enhanceApp({ app }) {
    app.component("HomePage", HomePage);
    app.component("MermaidDiagram", MermaidDiagram);

    if (typeof window !== "undefined") {
      window.addEventListener("scroll", () => {
        if (window.scrollY > 50) {
          document.documentElement.classList.add("is-scrolled");
        } else {
          document.documentElement.classList.remove("is-scrolled");
        }
      });
    }
  },
  setup() {
    if (typeof window !== "undefined") {
      const route = useRoute();
      watch(() => route.path, (path) => {
        if (path === "/" || path === "/index.html") {
          document.documentElement.classList.add("is-home-page");
        } else {
          document.documentElement.classList.remove("is-home-page");
        }
      }, { immediate: true });
    }
  }
};
