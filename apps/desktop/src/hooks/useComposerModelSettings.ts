import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import {
  getHarnessModelSelection,
  getSettings,
  listActiveHarnessModels,
  selectHarnessModel,
  SETTINGS_CHANGED_EVENT,
} from "../api";
import type { HarnessActiveModelView } from "../api";
import { message } from "../components/message";
import type { SettingsView } from "../types";
import type { HarnessThinkingLevel } from "../harnessModelsApi";

type BackendEffort = SettingsView["reasoning_effort"];
export type ThinkingLevel = HarnessThinkingLevel;
export type ThinkingSelection = ThinkingLevel | null;
type Choice<T> = { revision: number; value: T };
type Pending = { model?: Choice<string>; effort?: Choice<ThinkingLevel> & { model: string } };

function levelFromBackend(value: BackendEffort | null | undefined): ThinkingSelection {
  return value == null || value === "default" ? null
    : value === "none" ? "off" : value === "minimal" ? "low" : value === "xhigh" ? "max" : value;
}

function backendEffort(value: ThinkingLevel): string {
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
  defaultThinking: ThinkingSelection;
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
    thinkingLevels: model.thinkingDistinct.length ? model.thinkingDistinct : model.thinkingLevels,
    thinkingDistinct: model.thinkingDistinct,
    thinkingDisable: model.thinkingDisable,
    defaultThinking: levelFromBackend(model.defaultReasoningEffort as BackendEffort | null),
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
  const acceptedEffort = useRef<ThinkingSelection>(null);
  const revision = useRef(0);
  const readVersion = useRef(0);
  const draining = useRef(false);
  const mounted = useRef(false);
  const [state, setState] = useState({
    view: null as SettingsView | null,
    options: [] as ComposerModelOption[],
    activeOptions: [] as ComposerModelOption[],
    model: "",
    effort: null as ThinkingSelection,
    saving: false,
  });

  const publish = () => {
    if (!mounted.current) return;
    setState({
      view: current.current,
      options: optionsRef.current,
      activeOptions: catalog.current,
      model: pending.current.model?.value ?? acceptedModel.current ?? "",
      effort: pending.current.effort?.value ?? (pending.current.model
        ? optionsRef.current.find(option => option.id === pending.current.model?.value)?.defaultThinking ?? null
        : acceptedEffort.current),
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
    let active: ComposerModelOption[] | undefined;
    let selected: ComposerModelOption | null | undefined;
    let selectedEffort: string | undefined;
    let reported = false;
    const valid = () => mounted.current && version === readVersion.current && !draining.current;
    const reconcile = () => {
      if (!valid()) return;
      if (selected !== undefined) {
        selectedOption.current = selected;
        acceptedModel.current = selected?.id ?? "";
        acceptedEffort.current = levelFromBackend(selectedEffort as BackendEffort | undefined);
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
        current.current = result;
        reconcile();
      }).catch(error => failed("settings", error)),
      getHarnessModelSelection().then(result => {
        if (!valid()) return;
        selected = result ? optionFromActive(result) : null;
        selectedEffort = result?.reasoningEffort ?? result?.defaultReasoningEffort ?? undefined;
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
            acceptedEffort.current = levelFromBackend((receipt.reasoningEffort ?? receipt.defaultReasoningEffort) as BackendEffort | null);
            updateOptions();
            if (pending.current.model?.revision === model.revision && mounted.current) {
              message.success(t("composer.modelSwitched", { model: option?.label ?? model.value }));
            }
          } else {
            const option = optionsRef.current.find(candidate => candidate.id === effort!.model);
            if (!option || acceptedModel.current !== effort!.model) throw new Error("model selection changed before reasoning selection");
            const receipt = await selectHarnessModel(option.providerId, option.modelName, false, backendEffort(effort!.value));
            selectedOption.current = optionFromActive(receipt);
            acceptedEffort.current = levelFromBackend((receipt.reasoningEffort ?? receipt.defaultReasoningEffort) as BackendEffort | null);
            updateOptions();
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
    delete pending.current.effort;
    ++readVersion.current;
    publish();
    void drain();
  };

  const chooseThinking = (value: ThinkingLevel | "none") => {
    const level = value === "none" ? "off" : value;
    const model = pending.current.model?.value ?? acceptedModel.current;
    const option = optionsRef.current.find(candidate => candidate.id === model);
    if (!model || !option || !option.thinkingLevels.includes(level)) return;
    if (level === (pending.current.effort?.value ?? acceptedEffort.current)) return;
    pending.current.effort = { value: level, model, revision: ++revision.current };
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
    switching: state.saving,
    chooseModel,
    chooseThinking,
  };
}
