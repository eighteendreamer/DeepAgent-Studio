import type { PluginDefinition } from "./pluginTypes";
import { ComputerSettings } from "../settings/ComputerSettings";

export function ComputerUsePlugin() {
  return (
    <div className="h-full overflow-y-auto custom-scrollbar bg-white">
      <ComputerSettings />
    </div>
  );
}

export const computerUsePluginDefinition: PluginDefinition = {
  type: "computer_use",
  icon: ["fas", "desktop"],
  titleKey: "Computer Use",
  descKey: "Control desktop apps",
  fallbackTitle: "Computer Use",
  fallbackDesc: "Control desktop apps",
  getTabTitle: ({ t }) =>
    t?.("settings.computer.title", { defaultValue: "Computer Use" }) ||
    "Computer Use",
  render: () => <ComputerUsePlugin />,
};
