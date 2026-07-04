import { useEffect, useRef, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import { useTranslation } from "react-i18next";
import type { RecordingSession, TranscriptSegment } from "../../types";
import {
  audioPauseRecording,
  audioResumeRecording,
  audioStartRecording,
  audioStopRecording,
  officeExportMinutesDocx,
  runtimeInstall,
  runtimeProgressSubscribe,
  sendToChat,
  speechEngineInstalled,
  speechGenerateMeetingMinutes,
  speechModelInstalled,
  speechTranscribeFile,
} from "../../api";
import type { PluginDefinition } from "./pluginTypes";

const SPEECH_MODEL_ID = "whisper-base";
const SPEECH_ENGINE_ID = "whisper-cli";

type RuntimeDownloadState = {
  downloading: boolean;
  runtimeId: string | null;
  pct: number | null;
  label: string | null;
  promise: Promise<void> | null;
};

const runtimeDownloadListeners = new Set<() => void>();
let runtimeDownloadState: RuntimeDownloadState = {
  downloading: false,
  runtimeId: null,
  pct: null,
  label: null,
  promise: null,
};

function runtimeDownloadSnapshot() {
  return runtimeDownloadState;
}

function updateRuntimeDownload(patch: Partial<RuntimeDownloadState>) {
  runtimeDownloadState = { ...runtimeDownloadState, ...patch };
  runtimeDownloadListeners.forEach((listener) => listener());
}

function subscribeRuntimeDownload(listener: () => void) {
  runtimeDownloadListeners.add(listener);
  return () => {
    runtimeDownloadListeners.delete(listener);
  };
}

function installRuntimeWithProgress(runtimeId: string, label: string): Promise<void> {
  if (runtimeDownloadState.promise) return runtimeDownloadState.promise;
  const promise = (async () => {
    const unlisten = await runtimeProgressSubscribe(runtimeId, (p) => {
      if (p.total && p.total > 0) {
        updateRuntimeDownload({ pct: Math.round((p.downloaded / p.total) * 100) });
      }
    });
    try {
      await runtimeInstall(runtimeId);
      updateRuntimeDownload({ pct: 100 });
    } finally {
      unlisten();
      updateRuntimeDownload({
        downloading: false,
        runtimeId: null,
        pct: null,
        label: null,
        promise: null,
      });
    }
  })();
  updateRuntimeDownload({ downloading: true, runtimeId, pct: 0, label, promise });
  return promise;
}

/** Format milliseconds as mm:ss. */
function formatDuration(ms: number): string {
  const total = Math.floor(ms / 1000);
  const m = Math.floor(total / 60).toString().padStart(2, "0");
  const s = (total % 60).toString().padStart(2, "0");
  return `${m}:${s}`;
}

export function RecordingPlugin() {
  const { t } = useTranslation();
  const [session, setSession] = useState<RecordingSession | null>(null);
  const [elapsedMs, setElapsedMs] = useState(0);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // Transcription / minutes
  const [transcribing, setTranscribing] = useState(false);
  const [segments, setSegments] = useState<TranscriptSegment[] | null>(null);
  const [minutes, setMinutes] = useState<string | null>(null);
  const [generatingMinutes, setGeneratingMinutes] = useState(false);
  const [exportedPath, setExportedPath] = useState<string | null>(null);

  // Model download
  const [needModel, setNeedModel] = useState(false);
  const [needEngine, setNeedEngine] = useState(false);
  const [checkingDependencies, setCheckingDependencies] = useState(true);
  const [downloadState, setDownloadState] = useState(runtimeDownloadSnapshot);

  const tickRef = useRef<number | null>(null);

  const status = session?.status ?? "idle";
  const isRecording = status === "recording";
  const isPaused = status === "paused";
  const isActive = isRecording || isPaused;
  const downloading = downloadState.downloading;
  const downloadPct = downloadState.pct;
  const downloadLabel = downloadState.label;
  const engineDownloading = downloading && downloadState.runtimeId === SPEECH_ENGINE_ID;
  const modelDownloading = downloading && downloadState.runtimeId === SPEECH_MODEL_ID;
  const speechReady = !checkingDependencies && !needEngine && !needModel;

  useEffect(() => subscribeRuntimeDownload(() => setDownloadState(runtimeDownloadSnapshot())), []);

  useEffect(() => {
    if (!downloadState.downloading) return;
    if (downloadState.runtimeId === SPEECH_ENGINE_ID) setNeedEngine(true);
    if (downloadState.runtimeId === SPEECH_MODEL_ID) setNeedModel(true);
  }, [downloadState.downloading, downloadState.runtimeId]);

  const refreshSpeechDependencies = async () => {
    setCheckingDependencies(true);
    try {
      const [engineInstalled, modelInstalled] = await Promise.all([
        speechEngineInstalled(),
        speechModelInstalled(),
      ]);
      if (!runtimeDownloadSnapshot().downloading) {
        setNeedEngine(!engineInstalled);
        setNeedModel(!modelInstalled);
      }
      return engineInstalled && modelInstalled;
    } finally {
      setCheckingDependencies(false);
    }
  };

  useEffect(() => {
    let cancelled = false;
    setCheckingDependencies(true);
    Promise.all([speechEngineInstalled(), speechModelInstalled()])
      .then(([engineInstalled, modelInstalled]) => {
        if (cancelled || runtimeDownloadSnapshot().downloading) return;
        setNeedEngine(!engineInstalled);
        setNeedModel(!modelInstalled);
      })
      .catch((e) => {
        if (!cancelled) setError(String(e));
      })
      .finally(() => {
        if (!cancelled) setCheckingDependencies(false);
      });
    return () => {
      cancelled = true;
    };
  }, []);

  // Local 1s timer while recording (visual; duration is authoritative on stop).
  useEffect(() => {
    if (isRecording) {
      tickRef.current = window.setInterval(() => setElapsedMs((ms) => ms + 1000), 1000);
      return () => {
        if (tickRef.current != null) window.clearInterval(tickRef.current);
      };
    }
    return undefined;
  }, [isRecording]);

  const guard = async (fn: () => Promise<void>) => {
    setError(null);
    setBusy(true);
    try {
      await fn();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const start = () =>
    guard(async () => {
      if (!(await refreshSpeechDependencies())) return;
      setSegments(null);
      setMinutes(null);
      setElapsedMs(0);
      const name = `recording-${new Date().toISOString().slice(0, 16).replace(/[:T]/g, "")}`;
      const s = await audioStartRecording(name);
      setSession(s);
    });

  const pause = () =>
    guard(async () => setSession(await audioPauseRecording(session!.id)));
  const resume = () =>
    guard(async () => setSession(await audioResumeRecording(session!.id)));
  const stop = () =>
    guard(async () => {
      const s = await audioStopRecording(session!.id);
      setSession(s);
      setElapsedMs(s.duration_ms);
    });

  const runTranscription = async (audioPath: string) => {
    setTranscribing(true);
    setError(null);
    try {
      const segs = await speechTranscribeFile(audioPath);
      setSegments(segs);
      setNeedModel(false);
    } catch (e) {
      const msg = String(e);
      // The backend signals a missing model with a clear message.
      if (msg.includes("speech engine not installed") || msg.includes("speech engine executable missing")) {
        setNeedEngine(true);
      } else if (msg.includes("not installed") || msg.includes("model file missing")) {
        setNeedModel(true);
      } else {
        setError(msg);
      }
    } finally {
      setTranscribing(false);
    }
  };

  const transcribe = () =>
    guard(async () => {
      if (!session?.audio_path) return;
      const engineInstalled = await speechEngineInstalled();
      if (!engineInstalled) {
        setNeedEngine(true);
        return;
      }
      const installed = await speechModelInstalled();
      if (!installed) {
        setNeedModel(true);
        return;
      }
      await runTranscription(session.audio_path);
    });

  const downloadModel = () =>
    guard(async () => {
      await installRuntimeWithProgress(SPEECH_MODEL_ID, t("plugins.recording.downloadModel"));
      const ready = await refreshSpeechDependencies();
      if (ready && session?.audio_path) await runTranscription(session.audio_path);
    });

  const downloadEngine = () =>
    guard(async () => {
      await installRuntimeWithProgress(SPEECH_ENGINE_ID, "下载转写引擎");
      const ready = await refreshSpeechDependencies();
      if (ready && session?.audio_path) await transcribe();
    });

  const generateMinutes = () =>
    guard(async () => {
      const text = (segments ?? []).map((s) => s.text).join("\n").trim();
      if (!text) return;
      setGeneratingMinutes(true);
      try {
        const md = await speechGenerateMeetingMinutes(text);
        setMinutes(md);
        setExportedPath(null);
      } finally {
        setGeneratingMinutes(false);
      }
    });

  const exportWord = () =>
    guard(async () => {
      if (!minutes) return;
      const path = await officeExportMinutesDocx(minutes);
      setExportedPath(path);
    });

  const sendMinutesToChat = () => {
    if (!minutes) return;
    sendToChat(
      `<office-context>\n类型: 会议纪要\n${exportedPath ? `已导出: ${exportedPath}\n` : ""}内容:\n${minutes}\n</office-context>\n\n请基于以上会议纪要继续协助我。`
    );
  };

  const sendTranscriptToChat = () => {
    const text = (segments ?? []).map((s) => s.text).join("\n").trim();
    if (!text) return;
    sendToChat(
      `<office-context>\n类型: 录音转写\n内容:\n${text}\n</office-context>\n\n这是刚才的录音转写，请帮我整理或回答相关问题。`
    );
  };

  const statusLabel = t(`plugins.recording.${status}`);
  const statusColor =
    status === "recording" || status === "error"
      ? "text-red-500"
      : status === "done"
      ? "text-green-600"
      : "text-text-secondary";

  return (
    <div className="w-full h-full flex flex-col bg-white overflow-y-auto">
      <div className="flex-1 flex flex-col items-center p-8">
        <div className="w-full max-w-md flex flex-col items-center">
          {/* Status + timer */}
          <div className="flex items-center mb-2">
            {isRecording && (
              <span className="w-2.5 h-2.5 rounded-full bg-red-500 mr-2 animate-pulse" aria-hidden />
            )}
            <span className={`text-[13px] font-medium ${statusColor}`}>{statusLabel}</span>
          </div>
          <div className="text-5xl font-mono font-semibold text-text-base tabular-nums mb-8">
            {formatDuration(elapsedMs)}
          </div>

          {/* Primary controls */}
          <div className="flex items-center space-x-3 mb-6">
            {(status === "idle" || status === "done" || status === "error") && (
              <button
                type="button"
                onClick={start}
                disabled={busy || checkingDependencies || downloading || !speechReady}
                className="flex items-center px-5 py-2.5 rounded-full bg-primary text-white text-[14px] font-medium hover:opacity-90 transition-opacity disabled:opacity-50"
              >
                <FontAwesomeIcon icon={["fas", "microphone"]} className="mr-2" />
                {t("plugins.recording.start")}
              </button>
            )}
            {isRecording && (
              <button type="button" onClick={pause} disabled={busy} className="flex items-center px-5 py-2.5 rounded-full border border-border-theme text-text-base text-[14px] hover:border-primary/50 transition-colors disabled:opacity-50">
                <FontAwesomeIcon icon={["fas", "pause"]} className="mr-2" />
                {t("plugins.recording.pause")}
              </button>
            )}
            {isPaused && (
              <button type="button" onClick={resume} disabled={busy} className="flex items-center px-5 py-2.5 rounded-full border border-border-theme text-text-base text-[14px] hover:border-primary/50 transition-colors disabled:opacity-50">
                <FontAwesomeIcon icon={["fas", "play"]} className="mr-2" />
                {t("plugins.recording.resume")}
              </button>
            )}
            {isActive && (
              <button
                type="button"
                onClick={stop}
                disabled={busy}
                className="flex items-center px-5 py-2.5 rounded-full border border-red-300 text-red-500 text-[14px] hover:bg-red-50 transition-colors disabled:opacity-50"
              >
                <FontAwesomeIcon icon={["fas", "stop"]} className="mr-2" />
                {t("plugins.recording.stop")}
              </button>
            )}
          </div>

          {checkingDependencies && (
            <div className="w-full text-[12px] text-text-secondary text-center mb-4">
              正在检测语音模型...
            </div>
          )}

          {error && (
            <div className="w-full text-[12px] text-red-500 bg-red-50 border border-red-200 rounded-lg px-3 py-2 mb-4 whitespace-pre-wrap">
              {error}
            </div>
          )}

          {/* Post-recording actions */}
          {status === "done" && (
            <div className="w-full flex flex-col space-y-2">
              <ActionButton
                icon={["fas", "file-lines"]}
                label={t("plugins.recording.transcribe")}
                onClick={transcribe}
                disabled={busy || transcribing || checkingDependencies || !speechReady}
                spinning={transcribing}
              />
              {segments && segments.length > 0 && (
                <ActionButton
                  icon={["fas", "list-check"]}
                  label={t("plugins.recording.generateMinutes")}
                  onClick={generateMinutes}
                  disabled={busy || generatingMinutes}
                  spinning={generatingMinutes}
                />
              )}
            </div>
          )}

          {/* Engine download prompt */}
          {needEngine && (
            <div className="w-full mt-4 border border-border-theme rounded-xl p-4">
              <div className="text-[13px] text-text-base mb-1 font-medium">
                需要转写引擎
              </div>
              <div className="text-[12px] text-text-secondary mb-3">
                转写需要 whisper.cpp 本地引擎。引擎会下载到应用自身目录，并在使用前校验。
              </div>
              {engineDownloading ? (
                <div>
                  <div className="h-1.5 bg-gray-100 rounded-full overflow-hidden">
                    <div
                      className="h-full bg-primary transition-all"
                      style={{ width: `${downloadPct ?? 0}%` }}
                    />
                  </div>
                  <div className="text-[11px] text-text-secondary mt-1">
                    {downloadLabel ?? t("plugins.recording.downloading")} {downloadPct ?? 0}%
                  </div>
                </div>
              ) : (
                <button
                  type="button"
                  onClick={downloadEngine}
                  disabled={downloading}
                  className="flex items-center px-4 py-2 rounded-lg bg-primary text-white text-[13px] hover:opacity-90 disabled:opacity-50"
                >
                  <FontAwesomeIcon icon={["fas", "download"]} className="mr-2" />
                  下载转写引擎
                </button>
              )}
            </div>
          )}

          {/* Model download prompt */}
          {needModel && (
            <div className="w-full mt-4 border border-border-theme rounded-xl p-4">
              <div className="text-[13px] text-text-base mb-1 font-medium">
                {t("plugins.recording.modelNeededTitle")}
              </div>
              <div className="text-[12px] text-text-secondary mb-3">
                {t("plugins.recording.modelNeededDesc")}
              </div>
              {modelDownloading ? (
                <div>
                  <div className="h-1.5 bg-gray-100 rounded-full overflow-hidden">
                    <div
                      className="h-full bg-primary transition-all"
                      style={{ width: `${downloadPct ?? 0}%` }}
                    />
                  </div>
                  <div className="text-[11px] text-text-secondary mt-1">
                    {downloadLabel ?? t("plugins.recording.downloading")} {downloadPct ?? 0}%
                  </div>
                </div>
              ) : (
                <button
                  type="button"
                  onClick={downloadModel}
                  disabled={downloading}
                  className="flex items-center px-4 py-2 rounded-lg bg-primary text-white text-[13px] hover:opacity-90 disabled:opacity-50"
                >
                  <FontAwesomeIcon icon={["fas", "download"]} className="mr-2" />
                  {t("plugins.recording.downloadModel")}
                </button>
              )}
            </div>
          )}

          {/* Transcript segments */}
          {segments && (
            <div className="w-full mt-6">
              <div className="flex items-center justify-between mb-2">
                <div className="text-[13px] font-semibold text-text-base">
                  {t("plugins.recording.transcript")}
                </div>
                {segments.length > 0 && (
                  <button
                    type="button"
                    onClick={sendTranscriptToChat}
                    className="text-[12px] text-primary hover:underline"
                  >
                    {t("plugins.recording.sendToChat")}
                  </button>
                )}
              </div>
              {segments.length === 0 ? (
                <div className="text-[12px] text-text-secondary">
                  {t("plugins.recording.noSpeech")}
                </div>
              ) : (
                <div className="space-y-1 max-h-48 overflow-y-auto border border-border-theme rounded-lg p-3">
                  {segments.map((s, i) => (
                    <div key={i} className="text-[12px] text-text-base">
                      <span className="text-text-secondary font-mono mr-2">
                        {formatDuration(s.start_ms)}
                      </span>
                      {s.text}
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}

          {/* Minutes */}
          {minutes && (
            <div className="w-full mt-6">
              <div className="flex items-center justify-between mb-2">
                <div className="text-[13px] font-semibold text-text-base">
                  {t("plugins.recording.minutes")}
                </div>
                <div className="flex items-center space-x-3">
                  <button
                    type="button"
                    onClick={exportWord}
                    disabled={busy}
                    className="text-[12px] text-primary hover:underline disabled:opacity-50"
                  >
                    {t("plugins.recording.exportWord")}
                  </button>
                  <button
                    type="button"
                    onClick={sendMinutesToChat}
                    className="text-[12px] text-primary hover:underline"
                  >
                    {t("plugins.recording.sendToChat")}
                  </button>
                </div>
              </div>
              {exportedPath && (
                <div className="text-[11px] text-green-600 mb-2 break-all">
                  {t("plugins.recording.exportedTo")} {exportedPath}
                </div>
              )}
              <pre className="text-[12px] text-text-base whitespace-pre-wrap font-sans border border-border-theme rounded-lg p-3">
                {minutes}
              </pre>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

export const recordingPluginDefinition: PluginDefinition = {
  type: "recording",
  icon: ["fas", "microphone"],
  titleKey: "recording",
  descKey: "recordingDesc",
  fallbackTitle: "Recording",
  fallbackDesc: "Meeting recording and transcription",
  getTabTitle: ({ t }) =>
    t?.("chatView.tools.recording", { defaultValue: "Recording" }) || "Recording",
  render: () => <RecordingPlugin />,
};

function ActionButton({
  icon,
  label,
  onClick,
  disabled,
  spinning,
}: {
  icon: IconProp;
  label: string;
  onClick: () => void;
  disabled?: boolean;
  spinning?: boolean;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      className="flex items-center justify-center px-4 py-2.5 rounded-lg border border-border-theme text-text-base text-[13px] hover:border-primary/50 hover:text-primary transition-colors disabled:opacity-50"
    >
      <FontAwesomeIcon
        icon={spinning ? ["fas", "circle-notch"] : icon}
        className={`mr-2 ${spinning ? "animate-spin" : ""}`}
      />
      {label}
    </button>
  );
}
