import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  getHarnessModelSelection,
  getSettings,
  listActiveHarnessModels,
  selectHarnessModel,
  setReasoningEffort,
  SETTINGS_CHANGED_EVENT,
} from "../api";
import type { HarnessActiveModelView } from "../api";
import { message } from "../components/message";
import type { SettingsView } from "../types";

type BackendEffort = SettingsView["reasoning_effort"];
export type ThinkingLevel = "off" | "low" | "medium" | "high" | "max";
type Choice<T> = { revision: number; value: T };
type Pending = { model?: Choice<string>; effort?: Choice<ThinkingLevel> };

function levelFromBackend(value: BackendEffort): ThinkingLevel {
  return value === "none" ? "off" : value;
}

function backendEffort(value: ThinkingLevel): BackendEffort {
  return value === "off" ? "none" : value;
}

export interface ComposerModelOption {
  id: string;
  label: string;
  modelName: string;
  providerId: string;
  providerLabel: string;
  contextWindow: number;
  thinkingLevels: string[];
  thinkingDistinct: string[];
  thinkingDisable: boolean;
}

function optionId(model: HarnessActiveModelView): string {
  return JSON.stringify([model.providerId, model.modelName]);
}

function optionFromActive(model: HarnessActiveModelView): ComposerModelOption {
  return {
    id: optionId(model),
    label: model.displayName || model.modelName,
    modelName: model.modelName,
    providerId: model.providerId,
    providerLabel: model.providerChannel || model.providerId,
    contextWindow: model.contextWindow,
    thinkingLevels: model.thinkingLevels,
    thinkingDistinct: model.thinkingDistinct,
    thinkingDisable: model.thinkingDisable,
  };
}

/** Composer model state keeps the existing ordered settings writer intact. */
export function useComposerModelSettings() {
  const { t } = useTranslation();
  const current = useRef<SettingsView | null>(null);
  const optionsRef = useRef<ComposerModelOption[]>([]);
  const catalog = useRef<ComposerModelOption[]>([]);
  const selectedOption = useRef<ComposerModelOption | null>(null);
  const pending = useRef<Pending>({});
  const acceptedModel = useRef<string | null>(null);
  const acceptedEffort = useRef<ThinkingLevel | null>(null);
  const revision = useRef(0);
  const readVersion = useRef(0);
  const draining = useRef(false);
  const mounted = useRef(false);
  const [state, setState] = useState({
    view: null as SettingsView | null,
    options: [] as ComposerModelOption[],
    activeOptions: [] as ComposerModelOption[],
    model: "",
    effort: "high" as ThinkingLevel,
    saving: false,
  });

  const publish = () => {
    if (!mounted.current) return;
    setState({
      view: current.current,
      options: optionsRef.current,
      activeOptions: catalog.current,
      model: pending.current.model?.value ?? acceptedModel.current ?? "",
      effort: pending.current.effort?.value ?? acceptedEffort.current
        ?? (current.current ? levelFromBackend(current.current.reasoning_effort) : "high"),
      saving: draining.current,
    });
  };

  const updateOptions = () => {
    const selected = selectedOption.current;
    optionsRef.current = selected && !catalog.current.some(option => option.id === selected.id)
      ? [selected, ...catalog.current]
      : catalog.current;
  };

  const reload = async () => {
    const version = ++readVersion.current;
    let view: SettingsView | null | undefined;
    let active: ComposerModelOption[] | undefined;
    let selected: ComposerModelOption | null | undefined;
    let reported = false;
    const valid = () => mounted.current && version === readVersion.current && !draining.current;
    const reconcile = () => {
      if (!valid()) return;
      // Only a successful null selection permits the runtime's legacy DeepSeek
      // route. A pending or failed read must never become a different model.
      if (selected !== undefined && (selected !== null || (view !== undefined && active !== undefined))) {
        selectedOption.current = selected ?? active!.find(option =>
          option.providerId === "deepseek" && option.modelName === view?.chat_model) ?? null;
        acceptedModel.current = selectedOption.current?.id ?? "";
      }
      updateOptions();
      publish();
    };
    const failed = (source: string, error: unknown) => {
      if (!valid()) return;
      console.error(`load composer ${source} failed:`, error);
      if (!reported) message.error(t("composer.modelLoadFailed"));
      reported = true;
    };
    await Promise.all([
      getSettings().then(result => {
        if (!valid()) return;
        view = result;
        current.current = result;
        acceptedEffort.current = result ? levelFromBackend(result.reasoning_effort) : null;
        reconcile();
      }).catch(error => failed("settings", error)),
      getHarnessModelSelection().then(result => {
        if (!valid()) return;
        selected = result ? optionFromActive(result) : null;
        reconcile();
      }).catch(error => failed("selection", error)),
      listActiveHarnessModels().then(result => {
        if (!valid()) return;
        active = result.map(optionFromActive);
        catalog.current = active;
        reconcile();
      }).catch(error => failed("catalog", error)),
    ]);
  };

  useEffect(() => {
    mounted.current = true;
    void reload();
    const changed = (event: Event) => {
      if (draining.current) return;
      const reason = (event as CustomEvent<{ reason?: string }>).detail?.reason;
      if (reason && !["api_key", "models", "chat_model", "reasoning_effort", "model_capabilities", "slash_settings", "other"].includes(reason)) return;
      void reload();
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
          if (model) {
            const option = optionsRef.current.find((candidate) => candidate.id === model.value);
            if (!option) throw new Error("selected model is not in the available catalog");
            const previous = optionsRef.current.find((candidate) => candidate.id === acceptedModel.current);
            const receipt = await selectHarnessModel(option.providerId, option.modelName, previous?.providerId !== option.providerId);
            selectedOption.current = optionFromActive(receipt);
            acceptedModel.current = optionId(receipt);
            updateOptions();
            if (pending.current.model?.revision === model.revision && mounted.current) {
              message.success(t("composer.modelSwitched", { model: option?.label ?? model.value }));
            }
          } else {
            const receipt = await setReasoningEffort(backendEffort(effort!.value));
            current.current = receipt;
            acceptedEffort.current = effort!.value;
          }
        } catch (error) {
          console.error(model ? "set_harness_model failed:" : "set_reasoning_effort failed:", error);
          if (mounted.current) message.error(t(model ? "composer.modelSwitchFailed" : "composer.thinkingSwitchFailed"));
        }
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
      if (mounted.current) void reload();
    }
  };

  const chooseModel = (value: string) => {
    if (!optionsRef.current.some(option => option.id === value)
      || value === (pending.current.model?.value ?? acceptedModel.current)) return;
    pending.current.model = { value, revision: ++revision.current };
    ++readVersion.current;
    publish();
    void drain();
  };

  const chooseThinking = (value: ThinkingLevel | "none") => {
    const level = value === "none" ? "off" : value;
    if (level === (pending.current.effort?.value ?? acceptedEffort.current
      ?? (current.current ? levelFromBackend(current.current.reasoning_effort) : undefined))) return;
    pending.current.effort = { value: level, revision: ++revision.current };
    ++readVersion.current;
    publish();
    void drain();
  };

  const models = useMemo(() => state.options.map((option) => option.modelName), [state.options]);

  return {
    models,
    modelOptions: state.options,
    activeModelOptions: state.activeOptions,
    selectedModel: state.model,
    selectedThinking: state.effort,
    modelCapabilities: state.view?.model_capabilities ?? [],
    switching: state.saving,
    chooseModel,
    chooseThinking,
  };
}
