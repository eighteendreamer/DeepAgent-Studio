import type { ProjectMapStatus } from "../../types";
import { ProjectMapPanel } from "../project-map/ProjectMapPanel";
import type { PluginDefinition } from "./pluginTypes";

interface ProjectMapPluginProps {
  projectPath?: string | null;
  onStatusChange?: (status: ProjectMapStatus) => void;
}

export function ProjectMapPlugin({
  projectPath = null,
  onStatusChange,
}: ProjectMapPluginProps) {
  return <ProjectMapPanel projectPath={projectPath} onStatusChange={onStatusChange} />;
}

export const projectMapPluginDefinition: PluginDefinition = {
  type: "project_map",
  icon: ["fas", "share-nodes"],
  titleKey: "project_map",
  descKey: "projectMapDesc",
  fallbackTitle: "Project Map",
  fallbackDesc: "Inspect module relationships",
  getTabTitle: ({ t }) =>
    t?.("chatView.tools.project_map", { defaultValue: "Project Map" }) ||
    "Project Map",
  render: ({ activeProjectPath, onProjectMapStatusChange }) => (
    <ProjectMapPlugin
      projectPath={activeProjectPath}
      onStatusChange={onProjectMapStatusChange}
    />
  ),
};
