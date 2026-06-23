import { defineConfig } from "vitepress";

export default defineConfig({
  title: "DeepAgent Studio",
  description: "DeepSeek 原生 Agent 运行时平台与桌面 IDE",
  lang: "zh-CN",
  cleanUrls: false,
  appearance: false,
  head: [
    ["meta", { name: "theme-color", content: "#050505" }],
    ["meta", { property: "og:title", content: "DeepAgent Studio" }],
    ["meta", { property: "og:description", content: "可验证、可回放、可扩展的 Agent 运行时平台。" }],
    ["link", { rel: "icon", href: "/logo.png" }]
  ],
  themeConfig: {
    logo: "/logo.png",
    nav: [
      { text: "首页", link: "/" },
      { text: "文档", link: "/core/positioning" }
    ],
    sidebar: {
      "/": [
        {
          text: "核心概念",
          collapsed: false,
          items: [
            { text: "定位与技术栈", link: "/core/positioning" },
            { text: "架构与目录", link: "/core/architecture" },
            { text: "会话与数据模型", link: "/core/session" },
            { text: "运行循环与 DeepSeek", link: "/core/runtime" }
          ]
        },
        {
          text: "桌面端与工作流",
          collapsed: false,
          items: [
            { text: "桌面端与项目管理", link: "/desktop/app-structure" },
            { text: "代码图谱与项目地图", link: "/desktop/project-map" },
            { text: "Git 工作台", link: "/desktop/git" },
            { text: "文件、Office 与语音", link: "/desktop/office" },
            { text: "终端与归档", link: "/desktop/terminal-archive" }
          ]
        },
        {
          text: "扩展生态",
          collapsed: false,
          items: [
            { text: "工具系统", link: "/ecosystem/tools" },
            { text: "技能系统", link: "/ecosystem/skills" },
            { text: "知识库与记忆", link: "/ecosystem/knowledge" },
            { text: "MCP 远程协议", link: "/ecosystem/mcp" },
            { text: "子代理", link: "/ecosystem/subagents" }
          ]
        },
        {
          text: "安全与控制",
          collapsed: false,
          items: [
            { text: "沙箱、权限与 Hook", link: "/security/sandbox" },
            { text: "自愈验证", link: "/security/verification" },
            { text: "成本与预算", link: "/security/budget" }
          ]
        },
        {
          text: "开发者指南",
          collapsed: false,
          items: [
            { text: "Rust Crates 全览", link: "/dev/crates" },
            { text: "AppCore 服务门面", link: "/dev/services" },
            { text: "Tauri 与 Slash 命令", link: "/dev/commands" },
            { text: "开发工作流与 CI", link: "/dev/workflow" },
            { text: "排错与维护", link: "/dev/troubleshooting" },
            { text: "脚本与工作区资源", link: "/dev/resources" }
          ]
        }
      ]
    },
    socialLinks: [
      { icon: "github", link: "https://github.com/eighteendreamer/DeepAgent-Studio" }
    ],
    search: {
      provider: "local"
    },
    footer: {
      message: "DeepSeek 原生 Agent Runtime Operating System",
      copyright: "Apache-2.0 Licensed"
    }
  },
  markdown: {
    config(md) {
      const defaultFence = md.renderer.rules.fence;
      md.renderer.rules.fence = (tokens, idx, options, env, self) => {
        const token = tokens[idx];
        const info = token.info.trim();
        if (info === "mermaid") {
          return `<MermaidDiagram encoded="${encodeURIComponent(token.content)}" />`;
        }
        return defaultFence
          ? defaultFence(tokens, idx, options, env, self)
          : self.renderToken(tokens, idx, options);
      };
    }
  }
});
