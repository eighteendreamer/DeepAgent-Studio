import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

export type InstallOptions = {
  directory: string;
  desktop_shortcut: boolean;
  start_menu_shortcut: boolean;
  add_to_path: boolean;
  autostart: boolean;
};
export type Environment = {
  directory: string;
  available_bytes: number;
  required_bytes: number;
  webview_version: string | null;
  installed: { version: string; directory: string; machine: boolean } | null;
  target_version: string;
  payload_available: boolean;
};
export type InstallProgress = {
  phase: "preparing" | "installing" | "verifying" | "options" | "complete";
  percent: number;
  written_bytes: number;
  total_bytes: number;
};
export const checkEnvironment = (directory?: string) => invoke<Environment>("check_env", { directory });
export const chooseDirectory = () => invoke<string | null>("choose_directory");
export const install = (options: InstallOptions) => invoke<void>("install", { options });
export const applyOptions = () => invoke<void>("apply_options");
export const launchApp = () => invoke<void>("launch_app");
export const closeInstaller = () => invoke<void>("close_installer");
export const minimize = () => getCurrentWindow().minimize();
export const onProgress = (callback: (value: InstallProgress) => void) => listen<InstallProgress>("install-progress", event => callback(event.payload));
