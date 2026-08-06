import { lazy, Suspense } from "react";

const GeneralSettings = lazy(() => import("./settings/GeneralSettings").then((m) => ({ default: m.GeneralSettings })));
const AppearanceSettings = lazy(() => import("./settings/AppearanceSettings").then((m) => ({ default: m.AppearanceSettings })));
const ConfigSettings = lazy(() => import("./settings/ConfigSettings").then((m) => ({ default: m.ConfigSettings })));
const PersonalizeSettings = lazy(() => import("./settings/PersonalizeSettings").then((m) => ({ default: m.PersonalizeSettings })));
const ShortcutsSettings = lazy(() => import("./settings/ShortcutsSettings").then((m) => ({ default: m.ShortcutsSettings })));
const MCPSettings = lazy(() => import("./settings/MCPSettings").then((m) => ({ default: m.MCPSettings })));
const HooksSettings = lazy(() => import("./settings/HooksSettings").then((m) => ({ default: m.HooksSettings })));
const ConnectionsSettings = lazy(() => import("./settings/ConnectionsSettings").then((m) => ({ default: m.ConnectionsSettings })));
const GitSettings = lazy(() => import("./settings/GitSettings").then((m) => ({ default: m.GitSettings })));
const EnvSettings = lazy(() => import("./settings/EnvSettings").then((m) => ({ default: m.EnvSettings })));
const WorktreeSettings = lazy(() => import("./settings/WorktreeSettings").then((m) => ({ default: m.WorktreeSettings })));
const BrowserSettings = lazy(() => import("./settings/BrowserSettings").then((m) => ({ default: m.BrowserSettings })));
const ComputerSettings = lazy(() => import("./settings/ComputerSettings").then((m) => ({ default: m.ComputerSettings })));
const ArchiveSettings = lazy(() => import("./settings/ArchiveSettings").then((m) => ({ default: m.ArchiveSettings })));
const ProjectMapDebugSettings = lazy(() => import("./settings/ProjectMapDebugSettings").then((m) => ({ default: m.ProjectMapDebugSettings })));

interface Props {
  activeCategoryId: string;
}

export function SettingsView({ activeCategoryId }: Props) {
  const renderPlugin = () => {
    switch (activeCategoryId) {
      case "general": return <GeneralSettings />;
      case "appearance": return <AppearanceSettings />;
      case "config": return <ConfigSettings />;
      case "personalize": return <PersonalizeSettings />;
      case "shortcuts": return <ShortcutsSettings />;
      case "mcp": return <MCPSettings />;
      case "hooks": return <HooksSettings />;
      case "connections": return <ConnectionsSettings />;
      case "git": return <GitSettings />;
      case "env": return <EnvSettings />;
      case "worktree": return <WorktreeSettings />;
      case "browser": return <BrowserSettings />;
      case "computer": return <ComputerSettings />;
      case "project_map_debug": return <ProjectMapDebugSettings />;
      case "archive": return <ArchiveSettings />;
      default: return <GeneralSettings />;
    }
  };

  return (
    <div className="w-full h-full bg-bg-base text-text-base overflow-y-auto px-16 pt-16 pb-20 flex justify-center relative">
      <div className="w-full max-w-[700px]">
          <div
            key={activeCategoryId}
            className="settings-section w-full"
          >
            <Suspense
              fallback={
                <div className="flex min-h-40 items-center justify-center" aria-busy="true">
                  <div className="h-5 w-5 animate-spin rounded-full border-2 border-gray-200 border-t-primary" />
                </div>
              }
            >
              {renderPlugin()}
            </Suspense>
          </div>
      </div>
    </div>
  );
}
