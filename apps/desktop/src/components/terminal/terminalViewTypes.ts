import type { IntegrationShell } from "./terminalShellIntegration";
import type { ZmodemNotice } from "./terminalZmodem";
import type { TerminalControllerState } from "../plugins/terminalSessionController";
export type TerminalViewStatus = { state: TerminalControllerState; locked: boolean; hideOutput: boolean; recording: boolean; hasRecording: boolean; zmodem: ZmodemNotice };
export interface TerminalViewHandle {
  send(data: string): Promise<void>;
  paste(text: string, execute?: boolean): Promise<void>;
  lock(password: string, hideOutput: boolean): Promise<void>;
  unlock(password: string): Promise<void>;
  integrate(shell: IntegrationShell): Promise<void>;
  record(enabled: boolean): void;
  recording(): string | null;
  discardRecording(): void;
  focus(): void;
  confirmTransfer(): void;
  acceptTransfer(): Promise<void>;
  skipTransfer(): void;
  cancelTransfer(): void;
  sendFiles(files: File[]): Promise<void>;
}
