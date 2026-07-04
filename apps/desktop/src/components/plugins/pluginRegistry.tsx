import { browserPluginDefinition } from "./BrowserPlugin";
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

export function getPluginDefinition(type: PluginType): PluginDefinition | null {
  return PLUGIN_DEFINITION_MAP.get(type) ?? null;
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
