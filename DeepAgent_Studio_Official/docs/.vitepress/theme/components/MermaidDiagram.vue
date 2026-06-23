<script setup lang="ts">
import { onMounted, ref } from "vue";
import mermaid from "mermaid";

const props = defineProps<{
  encoded: string;
}>();

const el = ref<HTMLElement | null>(null);

onMounted(async () => {
  mermaid.initialize({
    startOnLoad: false,
    theme: "base",
    themeVariables: {
      background: "#ffffff",
      primaryColor: "#ffffff",
      primaryTextColor: "#080808",
      primaryBorderColor: "#111111",
      lineColor: "#111111",
      secondaryColor: "#f4f4f1",
      tertiaryColor: "#ffffff",
      fontFamily: "Inter, ui-sans-serif, system-ui, sans-serif"
    }
  });

  const code = decodeURIComponent(props.encoded);
  const id = `mermaid-${Math.random().toString(36).slice(2)}`;
  const result = await mermaid.render(id, code);
  if (el.value) {
    el.value.innerHTML = result.svg;
  }
});
</script>

<template>
  <div ref="el" class="mermaid-diagram" aria-label="Mermaid diagram"></div>
</template>
