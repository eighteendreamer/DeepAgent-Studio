import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import type { ReactNode } from "react";
import type { ProjectMapStatus } from "../../types";

export type PluginType =
  | "none"
  | "files"
  | "chat"
  | "browser"
  | "computer_use"
  | "terminal"
  | "project_map"
  | "recording"
  | "file_preview";

export type PluginTab = {
  id: string;
  type: PluginType;
  title: string;
  icon: IconProp;
  url?: string;
};

export type PluginToolCard = {
  icon: IconProp;
  title: string;
  desc: string;
  type: PluginType;
  id?: string;
  pluginId?: string;
  pluginAppId?: string;
  component?: string;
};

export type PluginConnectionSummary = {
  id?: string;
  name?: string | null;
  username?: string | null;
  host?: string | null;
};

export type PluginTitleContext = {
  activeProjectPath?: string | null;
  envMode?: "local" | "remote";
  selectedConnection?: PluginConnectionSummary | null;
  t?: (key: string, options?: Record<string, unknown>) => string;
};

export type PluginRenderContext = {
  activeProjectPath?: string | null;
  envMode?: "local" | "remote";
  selectedConnectionId?: string | null;
  onProjectMapStatusChange?: (status: ProjectMapStatus) => void;
};

export type CreatePluginTabOptions = {
  id?: string;
  title?: string;
  url?: string;
};

export type PluginDefinition = {
  type: PluginType;
  icon: IconProp;
  titleKey: string;
  descKey: string;
  fallbackTitle: string;
  fallbackDesc: string;
  getTabTitle?: (
    context: PluginTitleContext,
    options?: CreatePluginTabOptions,
  ) => string;
  render: (context: PluginRenderContext & { tab: PluginTab }) => ReactNode;
};
