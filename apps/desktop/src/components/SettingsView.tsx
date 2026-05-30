import { AnimatePresence, motion } from "framer-motion";
import { GeneralSettings } from "./settings/GeneralSettings";
import { AppearanceSettings } from "./settings/AppearanceSettings";
import { ConfigSettings } from "./settings/ConfigSettings";
import { PersonalizeSettings } from "./settings/PersonalizeSettings";
import { ShortcutsSettings } from "./settings/ShortcutsSettings";
import { MCPSettings } from "./settings/MCPSettings";
import { HooksSettings } from "./settings/HooksSettings";
import { ConnectionsSettings } from "./settings/ConnectionsSettings";
import { GitSettings } from "./settings/GitSettings";
import { EnvSettings } from "./settings/EnvSettings";
import { WorktreeSettings } from "./settings/WorktreeSettings";
import { BrowserSettings } from "./settings/BrowserSettings";
import { ComputerSettings } from "./settings/ComputerSettings";
import { ArchiveSettings } from "./settings/ArchiveSettings";

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
      case "archive": return <ArchiveSettings />;
      default: return <GeneralSettings />;
    }
  };

  return (
    <div className="w-full h-full bg-white overflow-y-auto px-16 pt-16 pb-20 flex justify-center relative">
      <div className="w-full max-w-[700px]">
        <AnimatePresence mode="wait">
          <motion.div
            key={activeCategoryId}
            initial={{ opacity: 0, y: 15 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -15 }}
            transition={{ type: "spring", bounce: 0, duration: 0.3 }}
            className="w-full"
          >
            {renderPlugin()}
          </motion.div>
        </AnimatePresence>
      </div>
    </div>
  );
}
