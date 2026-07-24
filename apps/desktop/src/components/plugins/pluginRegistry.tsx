import { lazy, Suspense, type ReactNode } from "react";
import type {
  CreatePluginTabOptions,
  PluginDefinition,
  PluginRenderContext,
  PluginTab,
  PluginTitleContext,
  PluginToolCard,
  PluginType,
} from "./pluginTypes";
import type { PluginApp } from "../../types";

const FilesPlugin = lazy(() => import("./FilesPlugin").then((m) => ({ default: m.FilesPlugin })));
const SideChatPlugin = lazy(() => import("./SideChatPlugin").then((m) => ({ default: m.SideChatPlugin })));
const BrowserPlugin = lazy(() => import("./BrowserPlugin").then((m) => ({ default: m.BrowserPlugin })));
const TerminalPlugin = lazy(() => import("./TerminalPlugin").then((m) => ({ default: m.TerminalPlugin })));
const ProjectMapPlugin = lazy(() => import("./ProjectMapPlugin").then((m) => ({ default: m.ProjectMapPlugin })));
const RecordingPlugin = lazy(() => import("./RecordingPlugin").then((m) => ({ default: m.RecordingPlugin })));
const FilePreviewPlugin = lazy(() => import("./FilePreviewPlugin").then((m) => ({ default: m.FilePreviewPlugin })));

function PluginLoading() {
  return (
    <div className="flex h-full w-full items-center justify-center bg-white" aria-busy="true">
      <div className="h-5 w-5 animate-spin rounded-full border-2 border-gray-200 border-t-primary" />
    </div>
  );
}

function renderLazyPlugin(node: ReactNode) {
  return <Suspense fallback={<PluginLoading />}>{node}</Suspense>;
}

function pathLabel(path: string | null | undefined): string {
  return path?.split(/[\\/]/).filter(Boolean).pop() ?? "";
}

function browserTitle(url: string | undefined): string {
  if (!url) return "";
  try {
    const normalized = /^https?:\/\//i.test(url) ? url : `https://${url}`;
    return new URL(normalized).host;
  } catch {
    return url;
  }
}

export type {
  PluginConnectionSummary,
  PluginDefinition,
  PluginRenderContext,
  PluginTab,
  PluginTitleContext,
  PluginToolCard,
  PluginType,
} from "./pluginTypes";

export const PLUGIN_DEFINITIONS: PluginDefinition[] = [
  {
    type: "files",
    icon: ["far", "folder-open"],
    titleKey: "files",
    descKey: "filesDesc",
    fallbackTitle: "Files",
    fallbackDesc: "Browse project files",
    getTabTitle: ({ activeProjectPath, t }) =>
      pathLabel(activeProjectPath) || t?.("chatView.tools.files", { defaultValue: "Files" }) || "Files",
    render: ({ activeProjectPath }) => renderLazyPlugin(<FilesPlugin projectPath={activeProjectPath} />),
  },
  {
    type: "chat",
    icon: ["far", "comment-dots"],
    titleKey: "chat",
    descKey: "chatDesc",
    fallbackTitle: "Side Chat",
    fallbackDesc: "Start a side chat",
    getTabTitle: ({ t }) => t?.("chatView.tools.chat", { defaultValue: "Side Chat" }) || "Side Chat",
    render: () => renderLazyPlugin(<SideChatPlugin />),
  },
  {
    type: "browser",
    icon: ["fas", "globe"],
    titleKey: "browser",
    descKey: "browserDesc",
    fallbackTitle: "Browser",
    fallbackDesc: "Open website",
    getTabTitle: ({ t }, options) =>
      browserTitle(options?.url) || t?.("chatView.tools.browser", { defaultValue: "Browser" }) || "Browser",
    render: ({ tab }) => renderLazyPlugin(<BrowserPlugin initialUrl={tab.url} />),
  },
  {
    type: "terminal",
    icon: ["fas", "terminal"],
    titleKey: "terminal",
    descKey: "terminalDesc",
    fallbackTitle: "Terminal",
    fallbackDesc: "Launch interactive shell",
    getTabTitle: ({ activeProjectPath, envMode = "local", selectedConnection }) => {
      if (envMode === "remote") {
        if (selectedConnection?.name) return selectedConnection.name;
        if (selectedConnection?.username && selectedConnection?.host) {
          return `${selectedConnection.username}@${selectedConnection.host}`;
        }
        return "SSH Terminal";
      }
      return activeProjectPath?.trim() || "Terminal";
    },
    render: ({ envMode = "local", selectedConnectionId }) =>
      renderLazyPlugin(<TerminalPlugin mode={envMode} connectionId={selectedConnectionId} />),
  },
  {
    type: "project_map",
    icon: ["fas", "share-nodes"],
    titleKey: "project_map",
    descKey: "projectMapDesc",
    fallbackTitle: "Project Map",
    fallbackDesc: "Inspect module relationships",
    getTabTitle: ({ t }) => t?.("chatView.tools.project_map", { defaultValue: "Project Map" }) || "Project Map",
    render: ({ activeProjectPath, onProjectMapStatusChange }) =>
      renderLazyPlugin(<ProjectMapPlugin projectPath={activeProjectPath} onStatusChange={onProjectMapStatusChange} />),
  },
  {
    type: "recording",
    icon: ["fas", "microphone"],
    titleKey: "recording",
    descKey: "recordingDesc",
    fallbackTitle: "Recording",
    fallbackDesc: "Meeting recording and transcription",
    getTabTitle: ({ t }) => t?.("chatView.tools.recording", { defaultValue: "Recording" }) || "Recording",
    render: () => renderLazyPlugin(<RecordingPlugin />),
  },
  {
    type: "file_preview",
    icon: ["far", "file-lines"],
    titleKey: "filePreview",
    descKey: "filePreviewDesc",
    fallbackTitle: "File Preview",
    fallbackDesc: "Preview office and project files",
    getTabTitle: ({ t }) => t?.("chatView.tools.file_preview", { defaultValue: "File Preview" }) || "File Preview",
    render: () => renderLazyPlugin(<FilePreviewPlugin />),
  },
];

const PLUGIN_DEFINITION_MAP = new Map(
  PLUGIN_DEFINITIONS.map((definition) => [definition.type, definition]),
);

export const PLUGIN_TOOL_CARDS: PluginToolCard[] = PLUGIN_DEFINITIONS.map(
  ({ icon, titleKey, descKey, type }) => ({
    icon,
    title: titleKey,
    desc: descKey,
    type,
  }),
);

const BUILTIN_COMPONENT_TYPES: Record<string, PluginType> = {
  files: "files",
  "side-chat": "chat",
  side_chat: "chat",
  chat: "chat",
  browser: "browser",
  terminal: "terminal",
  "project-map": "project_map",
  project_map: "project_map",
  recording: "recording",
  "file-preview": "file_preview",
  file_preview: "file_preview",
};

const PLUGIN_APP_ICON_MAP: Record<string, PluginToolCard["icon"]> = {
  files: ["far", "folder"],
  folder: ["far", "folder"],
  chat: ["far", "comments"],
  browser: ["fas", "arrow-pointer"],
  desktop: ["fas", "desktop"],
  terminal: ["fas", "terminal"],
  project_map: ["fas", "diagram-project"],
  "project-map": ["fas", "diagram-project"],
  recording: ["fas", "microphone"],
  file_preview: ["far", "file-lines"],
  "file-preview": ["far", "file-lines"],
  preview: ["far", "file-lines"],
};

export function getPluginDefinition(type: PluginType): PluginDefinition | null {
  return PLUGIN_DEFINITION_MAP.get(type) ?? null;
}

export function pluginAppToToolCard(app: PluginApp): PluginToolCard | null {
  const placement = (app.placement || "right-sidebar").trim().toLowerCase();
  if (placement !== "right-sidebar" && placement !== "sidebar") {
    return null;
  }

  const component = app.component.trim().toLowerCase();
  const builtinName = component.startsWith("builtin:")
    ? component.slice("builtin:".length)
    : component;
  const type = BUILTIN_COMPONENT_TYPES[builtinName];
  if (!type) {
    return null;
  }

  const definition = getPluginDefinition(type);
  const iconKey = (app.icon || builtinName).trim().toLowerCase();
  return {
    id: `plugin-app:${app.plugin_id}:${app.id}`,
    pluginId: app.plugin_id,
    pluginAppId: app.id,
    component: app.component,
    type,
    icon: PLUGIN_APP_ICON_MAP[iconKey] ?? definition?.icon ?? ["fas", "puzzle-piece"],
    title: app.title,
    desc: app.description || `${app.plugin_name} plugin app`,
  };
}

export function createPluginTab(
  type: PluginType,
  context: PluginTitleContext = {},
  options: CreatePluginTabOptions = {},
): PluginTab {
  const definition = getPluginDefinition(type);
  const title =
    options.title ??
    definition?.getTabTitle?.(context, options) ??
    definition?.fallbackTitle ??
    type;

  return {
    id: options.id ?? `${type}-${Date.now()}`,
    type,
    title,
    icon: definition?.icon ?? ["far", "square"],
    url: options.url,
  };
}

export function renderPluginTab(
  tab: PluginTab,
  context: PluginRenderContext = {},
) {
  return getPluginDefinition(tab.type)?.render({ ...context, tab }) ?? null;
}
