import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { getSettings, setChatModel, setReasoningEffort, SETTINGS_CHANGED_EVENT } from "../api";
import { message } from "../components/message";
import type { SettingsView } from "../types";

type Effort = SettingsView["reasoning_effort"];
type Choice<T> = { revision: number; value: T };
type Pending = { model?: Choice<string>; effort?: Choice<Effort> };

/** Optimistic UI with one ordered writer; only the latest unsaved choice is kept.
 * A slower settings read or save must never rewind a newer user selection.
 */
export function useComposerModelSettings() {
  const { t } = useTranslation();
  const current = useRef<SettingsView | null>(null);
  const pending = useRef<Pending>({});
  const revision = useRef(0);
  const readVersion = useRef(0);
  const draining = useRef(false);
  const mounted = useRef(false);
  const [state, setState] = useState({
    view: null as SettingsView | null, model: "", effort: "high" as Effort, saving: false,
  });

  const publish = () => {
    if (!mounted.current) return;
    setState({
      view: current.current,
      model: pending.current.model?.value ?? current.current?.chat_model ?? "",
      effort: pending.current.effort?.value ?? current.current?.reasoning_effort ?? "high",
      saving: draining.current,
    });
  };
  const reload = async () => {
    const version = ++readVersion.current;
    try {
      const view = await getSettings();
      if (!mounted.current || version !== readVersion.current || draining.current) return;
      current.current = view;
      publish();
    } catch {
      // Browser preview / unavailable settings: preserve the last known view.
    }
  };

  useEffect(() => {
    mounted.current = true;
    void reload();
    const changed = () => {
      // A setter emits before its response resolves. Its returned view is the
      // receipt; reconcile once after draining, not after every slider click.
      if (!draining.current) void reload();
    };
    window.addEventListener(SETTINGS_CHANGED_EVENT, changed);
    return () => {
      mounted.current = false;
      ++readVersion.current;
      window.removeEventListener(SETTINGS_CHANGED_EVENT, changed);
    };
  }, []);

  const drain = async () => {
    if (draining.current) return;
    draining.current = true;
    publish();
    try {
      while (pending.current.model || pending.current.effort) {
        const model = pending.current.model;
        const effort = pending.current.effort;
        try {
          const view = model ? await setChatModel(model.value) : await setReasoningEffort(effort!.value);
          current.current = view;
          if (model && pending.current.model?.revision === model.revision && mounted.current) {
            message.success(t("composer.modelSwitched", { model: view.chat_model }));
          }
        } catch (error) {
          console.error(model ? "set_chat_model failed:" : "set_reasoning_effort failed:", error);
          if (mounted.current) message.error(t(model ? "composer.modelSwitchFailed" : "composer.thinkingSwitchFailed"));
        }
        // Clear only the acknowledged intent. On failure the UI falls back to
        // the last saved view, unless a newer intent is still queued.
        if (model) {
          if (pending.current.model?.revision === model.revision) delete pending.current.model;
        } else if (pending.current.effort?.revision === effort!.revision) {
          delete pending.current.effort;
        }
        publish();
      }
    } finally {
      draining.current = false;
      publish();
      // Includes external settings changes received while our writer was busy.
      if (mounted.current) void reload();
    }
  };

  const chooseModel = (value: string) => {
    if (value === (pending.current.model?.value ?? current.current?.chat_model)) return;
    pending.current.model = { value, revision: ++revision.current };
    ++readVersion.current;
    publish();
    void drain();
  };
  const chooseThinking = (value: Effort) => {
    if (value === (pending.current.effort?.value ?? current.current?.reasoning_effort)) return;
    pending.current.effort = { value, revision: ++revision.current };
    ++readVersion.current;
    publish();
    void drain();
  };

  return {
    models: state.view?.available_models ?? [],
    modelCapabilities: state.view?.model_capabilities ?? [],
    selectedModel: state.model,
    selectedThinking: state.effort,
    switching: state.saving,
    chooseModel,
    chooseThinking,
  };
}
