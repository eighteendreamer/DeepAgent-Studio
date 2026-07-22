import { browserPluginDefinition } from "./BrowserPlugin";
import { computerUsePluginDefinition } from "./ComputerUsePlugin";
import { filePreviewPluginDefinition } from "./FilePreviewPlugin";
import { filesPluginDefinition } from "./FilesPlugin";
import { projectMapPluginDefinition } from "./ProjectMapPlugin";
import { recordingPluginDefinition } from "./RecordingPlugin";
import { sideChatPluginDefinition } from "./SideChatPlugin";
import { terminalPluginDefinition } from "./TerminalPlugin";
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
  filesPluginDefinition,
  sideChatPluginDefinition,
  browserPluginDefinition,
  computerUsePluginDefinition,
  terminalPluginDefinition,
  projectMapPluginDefinition,
  recordingPluginDefinition,
  filePreviewPluginDefinition,
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
  "computer-use": "computer_use",
  computer_use: "computer_use",
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
  "computer-use": ["fas", "desktop"],
  computer_use: ["fas", "desktop"],
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
