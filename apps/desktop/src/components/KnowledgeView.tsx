import { useEffect, useMemo, useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import { useTranslation } from "react-i18next";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import type { KnowledgeEntry, KnowledgeDraft } from "../types";
import {
  kbList,
  kbReload,
  kbSave,
  kbDelete,
  kbSetPassive,
  kbPassiveEnabled,
  kbListDrafts,
  kbAcceptDraft,
  kbDiscardDraft,
  kbSetAutoCapture,
  kbAutoCaptureEnabled,
} from "../api";
import { message } from "./message";
import { KnowledgeGraph } from "./KnowledgeGraph";

const KINDS = ["pitfall", "solution", "command", "config", "note"] as const;
type Kind = (typeof KINDS)[number];

// Map an entry kind to an icon + accent color.
function visualFor(kind: string): { icon: IconProp; bg: string } {
  const map: Record<string, { icon: IconProp; bg: string }> = {
    pitfall: { icon: ["fas", "triangle-exclamation"], bg: "bg-red-100 text-red-600" },
    solution: { icon: ["fas", "wrench"], bg: "bg-green-100 text-green-600" },
    command: { icon: ["fas", "terminal"], bg: "bg-gray-100 text-gray-700" },
    config: { icon: ["fas", "sliders"], bg: "bg-blue-100 text-blue-600" },
    note: { icon: ["fas", "note-sticky"], bg: "bg-yellow-100 text-yellow-700" },
  };
  return map[kind] ?? { icon: ["fas", "note-sticky"], bg: "bg-gray-100 text-gray-600" };
}

// Legend dot colors mirror KnowledgeGraph's KIND_COLOR.
const KIND_DOT: Record<string, string> = {
  pitfall: "bg-red-600",
  solution: "bg-green-600",
  command: "bg-gray-600",
  config: "bg-blue-600",
  note: "bg-yellow-600",
};

const EMPTY_DRAFT: KnowledgeDraft = {
  title: "",
  body: "",
  kind: "note",
  tags: [],
  scope: "project",
};

type ViewMode = "graph" | "list";

export function KnowledgeView() {
  const { t } = useTranslation();
  const [search, setSearch] = useState("");
  const [entries, setEntries] = useState<KnowledgeEntry[]>([]);
  const [loading, setLoading] = useState(true);
  const [selected, setSelected] = useState<KnowledgeEntry | null>(null);
  const [passive, setPassive] = useState(true);
  const [drafts, setDrafts] = useState<KnowledgeEntry[]>([]);
  const [autoCapture, setAutoCapture] = useState(true);
  const [mode, setMode] = useState<ViewMode>("graph");
  const [showDrafts, setShowDrafts] = useState(false);

  // Editor state (used for both create and edit).
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState<KnowledgeDraft>(EMPTY_DRAFT);
  const [tagsInput, setTagsInput] = useState("");

  async function refresh(rescan = false) {
    setLoading(true);
    try {
      const list = rescan ? await kbReload() : await kbList();
      setEntries(list);
      setDrafts(await kbListDrafts());
    } catch (e: any) {
      message.error(t("knowledgeView.loadFailed", { error: e?.message ?? e }));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh(false);
    kbPassiveEnabled().then(setPassive).catch(() => {});
    kbAutoCaptureEnabled().then(setAutoCapture).catch(() => {});
  }, []);

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return entries;
    return entries.filter(
      (e) =>
        e.title.toLowerCase().includes(q) ||
        e.body.toLowerCase().includes(q) ||
        e.tags.some((tag) => tag.toLowerCase().includes(q))
    );
  }, [entries, search]);

  // Distinct kinds present, for the graph legend.
  const presentKinds = useMemo(() => {
    const set = new Set<string>();
    entries.forEach((e) => set.add(e.kind));
    return KINDS.filter((k) => set.has(k));
  }, [entries]);

  function openNew() {
    setDraft(EMPTY_DRAFT);
    setTagsInput("");
    setSelected(null);
    setEditing(true);
  }

  function openEdit(entry: KnowledgeEntry) {
    setDraft({
      title: entry.title,
      body: entry.body,
      kind: entry.kind,
      tags: entry.tags,
      scope: entry.scope,
    });
    setTagsInput(entry.tags.join(", "));
    setSelected(entry);
    setEditing(true);
  }

  async function onTogglePassive() {
    const next = !passive;
    try {
      const state = await kbSetPassive(next);
      setPassive(state);
      message.success(state ? t("knowledgeView.passiveOn") : t("knowledgeView.passiveOff"));
    } catch (e: any) {
      message.error(t("knowledgeView.saveFailed", { error: e?.message ?? e }));
    }
  }

  async function onToggleAutoCapture() {
    const next = !autoCapture;
    try {
      const state = await kbSetAutoCapture(next);
      setAutoCapture(state);
      message.success(state ? t("knowledgeView.autoCaptureOn") : t("knowledgeView.autoCaptureOff"));
    } catch (e: any) {
      message.error(t("knowledgeView.saveFailed", { error: e?.message ?? e }));
    }
  }

  async function onAcceptDraft(draftEntry: KnowledgeEntry) {
    try {
      await kbAcceptDraft(draftEntry.id);
      message.success(t("knowledgeView.draftAccepted"));
      await refresh(false);
    } catch (e: any) {
      message.error(t("knowledgeView.saveFailed", { error: e?.message ?? e }));
    }
  }

  async function onDiscardDraft(draftEntry: KnowledgeEntry) {
    try {
      await kbDiscardDraft(draftEntry.id);
      message.success(t("knowledgeView.draftDiscarded"));
      await refresh(false);
    } catch (e: any) {
      message.error(t("knowledgeView.deleteFailed", { error: e?.message ?? e }));
    }
  }

  async function onSave() {
    if (!draft.title.trim()) {
      message.error(t("knowledgeView.titleRequired"));
      return;
    }
    if (!draft.body.trim()) {
      message.error(t("knowledgeView.bodyRequired"));
      return;
    }
    const tags = tagsInput
      .split(",")
      .map((s) => s.trim())
      .filter(Boolean);
    try {
      const saved = await kbSave({ ...draft, tags });
      message.success(t("knowledgeView.saved"));
      setEditing(false);
      setSelected(saved);
      await refresh(false);
    } catch (e: any) {
      message.error(t("knowledgeView.saveFailed", { error: e?.message ?? e }));
    }
  }

  async function onDelete(entry: KnowledgeEntry) {
    try {
      await kbDelete(entry.id);
      message.success(t("knowledgeView.deleted"));
      if (selected?.id === entry.id) {
        setSelected(null);
        setEditing(false);
      }
      await refresh(false);
    } catch (e: any) {
      message.error(t("knowledgeView.deleteFailed", { error: e?.message ?? e }));
    }
  }

  const iconBtn =
    "flex items-center justify-center w-8 h-8 rounded border border-border-theme text-text-secondary hover:text-text-base hover:bg-gray-50 transition-colors";

  return (
    <div className="w-full h-full flex bg-white overflow-hidden">
      {/* Main area: graph (or list) */}
      <div className="flex-1 flex flex-col overflow-hidden">
        {/* Toolbar */}
        <div className="flex items-center justify-between px-6 py-4 border-b border-border-theme flex-shrink-0">
          <div className="min-w-0">
            <h1 className="text-xl font-semibold text-text-base leading-tight">{t("knowledgeView.title")}</h1>
            <p className="text-xs text-text-secondary truncate">
              {t("knowledgeView.subtitle1")}
              <code className="text-[11px] bg-gray-100 px-1 py-0.5 rounded">.deepagent/knowledge</code>
              {t("knowledgeView.subtitle2")}
            </p>
          </div>

          <div className="flex items-center gap-2 flex-shrink-0">
            {/* graph / list switch */}
            <div className="flex items-center bg-gray-100 rounded-lg p-0.5">
              <button
                onClick={() => setMode("graph")}
                className={`px-2.5 py-1 text-xs rounded-md transition-colors flex items-center gap-1.5 ${
                  mode === "graph" ? "bg-white text-text-base shadow-sm" : "text-text-secondary hover:text-text-base"
                }`}
                title={t("knowledgeView.graphView")}
              >
                <FontAwesomeIcon icon={["fas", "share-nodes"]} />
                {t("knowledgeView.graphView")}
              </button>
              <button
                onClick={() => setMode("list")}
                className={`px-2.5 py-1 text-xs rounded-md transition-colors flex items-center gap-1.5 ${
                  mode === "list" ? "bg-white text-text-base shadow-sm" : "text-text-secondary hover:text-text-base"
                }`}
                title={t("knowledgeView.listView")}
              >
                <FontAwesomeIcon icon={["fas", "list"]} />
                {t("knowledgeView.listView")}
              </button>
            </div>

            <div className="flex items-center bg-gray-50 border border-border-theme rounded-full px-3 py-1.5 w-56 focus-within:border-gray-300 focus-within:bg-white transition-all">
              <FontAwesomeIcon icon={["fas", "magnifying-glass"]} className="text-text-secondary text-sm mr-2" />
              <input
                type="text"
                placeholder={t("knowledgeView.searchPlaceholder")}
                className="bg-transparent outline-none w-full text-sm text-text-base"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
              />
            </div>

            {/* drafts bell */}
            <button
              onClick={() => setShowDrafts((s) => !s)}
              className={`relative ${iconBtn} ${showDrafts ? "bg-amber-50 border-amber-200 text-amber-600" : ""}`}
              title={t("knowledgeView.draftsTitle", { count: drafts.length })}
            >
              <FontAwesomeIcon icon={["fas", "inbox"]} className="text-sm" />
              {drafts.length > 0 && (
                <span className="absolute -top-1.5 -right-1.5 min-w-[16px] h-4 px-1 rounded-full bg-amber-500 text-white text-[10px] leading-4 text-center">
                  {drafts.length}
                </span>
              )}
            </button>

            <button onClick={() => refresh(true)} className={iconBtn} title={t("knowledgeView.refresh")}>
              <FontAwesomeIcon icon={["fas", "rotate-right"]} className={`text-sm ${loading ? "animate-spin" : ""}`} />
            </button>
            <button onClick={openNew} className={iconBtn} title={t("knowledgeView.new")}>
              <FontAwesomeIcon icon={["fas", "plus"]} className="text-sm" />
            </button>
          </div>
        </div>

        {/* Secondary bar: toggles + legend */}
        <div className="flex items-center justify-between px-6 py-2 border-b border-border-theme flex-shrink-0 bg-gray-50/40">
          <div className="flex items-center gap-4">
            <button
              onClick={onTogglePassive}
              className={`flex items-center text-xs cursor-pointer transition-colors ${
                passive ? "text-green-600 hover:text-green-700" : "text-text-secondary hover:text-text-base"
              }`}
              title={t("knowledgeView.passiveHint")}
            >
              <FontAwesomeIcon icon={["fas", passive ? "toggle-on" : "toggle-off"]} className="mr-1.5 text-sm" />
              {t("knowledgeView.passive")}
            </button>
            <button
              onClick={onToggleAutoCapture}
              className={`flex items-center text-xs cursor-pointer transition-colors ${
                autoCapture ? "text-green-600 hover:text-green-700" : "text-text-secondary hover:text-text-base"
              }`}
              title={t("knowledgeView.autoCaptureHint")}
            >
              <FontAwesomeIcon icon={["fas", autoCapture ? "toggle-on" : "toggle-off"]} className="mr-1.5 text-sm" />
              {t("knowledgeView.autoCapture")}
            </button>
            <span className="text-xs text-text-secondary">
              {t("knowledgeView.count", { count: filtered.length })}
            </span>
          </div>

          {mode === "graph" && presentKinds.length > 0 && (
            <div className="flex items-center gap-3">
              {presentKinds.map((k) => (
                <span key={k} className="flex items-center text-[11px] text-text-secondary">
                  <span className={`w-2.5 h-2.5 rounded-full mr-1.5 ${KIND_DOT[k]}`} />
                  {t(`knowledgeView.kind.${k}`)}
                </span>
              ))}
              <span className="flex items-center text-[11px] text-text-secondary">
                <span className="w-2.5 h-2.5 rounded-full mr-1.5 border border-slate-400 bg-white" />
                {t("knowledgeView.tagNode")}
              </span>
            </div>
          )}
        </div>

        {/* Content */}
        <div className="flex-1 min-h-0 relative">
          {loading && entries.length === 0 ? (
            <div className="text-sm text-text-secondary py-10 text-center">{t("knowledgeView.loading")}</div>
          ) : entries.length === 0 ? (
            <div className="h-full flex flex-col items-center justify-center text-center px-6">
              <FontAwesomeIcon icon={["fas", "share-nodes"]} className="text-4xl text-gray-300 mb-3" />
              <div className="text-sm text-text-secondary">{t("knowledgeView.empty")}</div>
            </div>
          ) : mode === "graph" ? (
            <KnowledgeGraph
              entries={filtered}
              selectedId={selected?.id ?? null}
              search={search}
              onSelect={(en) => {
                setSelected(en);
                setEditing(false);
              }}
            />
          ) : (
            <div className="h-full overflow-y-auto px-6 py-5">
              <div className="grid grid-cols-2 gap-x-6 gap-y-2 max-w-4xl mx-auto">
                {filtered.map((entry) => {
                  const v = visualFor(entry.kind);
                  const active = selected?.id === entry.id;
                  return (
                    <div
                      key={entry.id}
                      onClick={() => {
                        setSelected(entry);
                        setEditing(false);
                      }}
                      className={`flex items-center p-3 rounded-xl cursor-pointer transition-colors group ${
                        active ? "bg-gray-100" : "hover:bg-gray-50"
                      }`}
                    >
                      <div className={`w-10 h-10 rounded-lg flex items-center justify-center flex-shrink-0 mr-4 ${v.bg}`}>
                        <FontAwesomeIcon icon={v.icon} className="text-lg" />
                      </div>
                      <div className="flex-1 min-w-0 pr-3">
                        <div className="flex items-center gap-2">
                          <span className="text-[14px] font-medium text-text-base truncate">{entry.title}</span>
                          <span className="text-[10px] text-text-secondary border border-border-theme rounded-full px-1.5 py-0.5 flex-shrink-0">
                            {entry.scope}
                          </span>
                        </div>
                        <div className="text-[12px] text-text-secondary truncate mt-0.5">{entry.body}</div>
                      </div>
                      <button
                        onClick={(e) => {
                          e.stopPropagation();
                          onDelete(entry);
                        }}
                        title={t("knowledgeView.delete")}
                        className="w-7 h-7 rounded-full border border-border-theme flex items-center justify-center text-text-secondary hover:bg-white hover:text-red-500 transition-all bg-gray-50 opacity-0 group-hover:opacity-100"
                      >
                        <FontAwesomeIcon icon={["fas", "trash"]} className="text-xs" />
                      </button>
                    </div>
                  );
                })}
              </div>
            </div>
          )}

          {/* Drafts popover (overlays the content area) */}
          {showDrafts && (
            <div className="absolute top-3 right-3 z-20 w-96 max-h-[70%] overflow-y-auto bg-white border border-amber-200 rounded-xl shadow-lg p-4">
              <div className="flex items-center justify-between mb-3">
                <div className="flex items-center">
                  <FontAwesomeIcon icon={["fas", "triangle-exclamation"]} className="text-amber-500 mr-2" />
                  <h2 className="text-sm font-medium text-text-base">
                    {t("knowledgeView.draftsTitle", { count: drafts.length })}
                  </h2>
                </div>
                <button onClick={() => setShowDrafts(false)} className="text-text-secondary hover:text-text-base">
                  <FontAwesomeIcon icon={["fas", "xmark"]} />
                </button>
              </div>
              <p className="text-xs text-text-secondary mb-3">{t("knowledgeView.draftsHint")}</p>
              {drafts.length === 0 ? (
                <div className="text-xs text-text-secondary py-6 text-center">{t("knowledgeView.noDrafts")}</div>
              ) : (
                <div className="space-y-2">
                  {drafts.map((d) => (
                    <div key={d.id} className="bg-amber-50/60 border border-amber-200 rounded-lg p-3">
                      <div className="flex items-center gap-2 mb-1">
                        <span className="text-[13px] font-medium text-text-base truncate">{d.title}</span>
                        <span className="text-[10px] text-amber-600 border border-amber-300 rounded-full px-1.5 py-0.5 flex-shrink-0">
                          {t(`knowledgeView.kind.${d.kind}`, d.kind)}
                        </span>
                      </div>
                      <div className="text-[12px] text-text-secondary line-clamp-3 mb-2">{d.body}</div>
                      <div className="flex items-center justify-end gap-1.5">
                        <button
                          onClick={() => onAcceptDraft(d)}
                          className="px-2.5 py-1 text-xs rounded-md bg-green-600 text-white hover:bg-green-700 transition-colors"
                        >
                          {t("knowledgeView.accept")}
                        </button>
                        <button
                          onClick={() => onDiscardDraft(d)}
                          className="px-2.5 py-1 text-xs rounded-md border border-border-theme text-text-secondary hover:bg-gray-50 transition-colors"
                        >
                          {t("knowledgeView.discard")}
                        </button>
                      </div>
                    </div>
                  ))}
                </div>
              )}
            </div>
          )}
        </div>
      </div>

      {/* Right: detail or editor */}
      {(selected || editing) && (
        <div className="w-96 border-l border-border-theme flex flex-col overflow-hidden bg-gray-50/50 flex-shrink-0">
          {editing ? (
            <>
              <div className="px-6 py-5 border-b border-border-theme flex items-start justify-between">
                <div className="text-lg font-semibold text-text-base">
                  {selected ? t("knowledgeView.editTitle") : t("knowledgeView.newTitle")}
                </div>
                <button onClick={() => setEditing(false)} className="text-text-secondary hover:text-text-base">
                  <FontAwesomeIcon icon={["fas", "xmark"]} />
                </button>
              </div>
              <div className="flex-1 overflow-y-auto px-6 py-4 space-y-4">
                <div>
                  <div className="text-xs font-medium text-text-secondary uppercase tracking-wide mb-1.5">
                    {t("knowledgeView.fieldTitle")}
                  </div>
                  <input
                    type="text"
                    className="w-full text-sm bg-white border border-border-theme rounded-lg px-3 py-2 outline-none focus:border-gray-300 text-text-base"
                    value={draft.title}
                    onChange={(e) => setDraft({ ...draft, title: e.target.value })}
                    placeholder={t("knowledgeView.fieldTitlePlaceholder")}
                  />
                </div>
                <div className="flex gap-3">
                  <div className="flex-1">
                    <div className="text-xs font-medium text-text-secondary uppercase tracking-wide mb-1.5">
                      {t("knowledgeView.fieldKind")}
                    </div>
                    <select
                      className="w-full text-sm bg-white border border-border-theme rounded-lg px-3 py-2 outline-none focus:border-gray-300 text-text-base"
                      value={draft.kind ?? "note"}
                      onChange={(e) => setDraft({ ...draft, kind: e.target.value as Kind })}
                    >
                      {KINDS.map((k) => (
                        <option key={k} value={k}>
                          {t(`knowledgeView.kind.${k}`)}
                        </option>
                      ))}
                    </select>
                  </div>
                  <div className="flex-1">
                    <div className="text-xs font-medium text-text-secondary uppercase tracking-wide mb-1.5">
                      {t("knowledgeView.fieldScope")}
                    </div>
                    <select
                      className="w-full text-sm bg-white border border-border-theme rounded-lg px-3 py-2 outline-none focus:border-gray-300 text-text-base"
                      value={draft.scope ?? "project"}
                      onChange={(e) => setDraft({ ...draft, scope: e.target.value })}
                      disabled={!!selected}
                    >
                      <option value="project">{t("knowledgeView.scope.project")}</option>
                      <option value="global">{t("knowledgeView.scope.global")}</option>
                    </select>
                  </div>
                </div>
                <div>
                  <div className="text-xs font-medium text-text-secondary uppercase tracking-wide mb-1.5">
                    {t("knowledgeView.fieldTags")}
                  </div>
                  <input
                    type="text"
                    className="w-full text-sm bg-white border border-border-theme rounded-lg px-3 py-2 outline-none focus:border-gray-300 text-text-base"
                    value={tagsInput}
                    onChange={(e) => setTagsInput(e.target.value)}
                    placeholder={t("knowledgeView.fieldTagsPlaceholder")}
                  />
                </div>
                <div>
                  <div className="text-xs font-medium text-text-secondary uppercase tracking-wide mb-1.5">
                    {t("knowledgeView.fieldBody")}
                  </div>
                  <textarea
                    className="w-full h-48 text-[12px] font-mono leading-relaxed bg-white border border-border-theme rounded-lg px-3 py-2 outline-none focus:border-gray-300 text-text-base resize-none"
                    value={draft.body}
                    onChange={(e) => setDraft({ ...draft, body: e.target.value })}
                    placeholder={t("knowledgeView.fieldBodyPlaceholder")}
                  />
                </div>
              </div>
              <div className="px-6 py-4 border-t border-border-theme flex justify-end gap-2">
                <button
                  onClick={() => setEditing(false)}
                  className="px-4 py-1.5 text-sm rounded-lg border border-border-theme text-text-secondary hover:bg-gray-50 transition-colors"
                >
                  {t("knowledgeView.cancel")}
                </button>
                <button
                  onClick={onSave}
                  className="px-4 py-1.5 text-sm rounded-lg bg-text-base text-white hover:opacity-90 transition-opacity"
                >
                  {t("knowledgeView.save")}
                </button>
              </div>
            </>
          ) : selected ? (
            <>
              <div className="px-6 py-5 border-b border-border-theme flex items-start justify-between">
                <div>
                  <div className="text-lg font-semibold text-text-base">{selected.title}</div>
                  <div className="text-xs text-text-secondary mt-0.5">
                    {t(`knowledgeView.kind.${selected.kind}`, selected.kind)} · {selected.scope}
                  </div>
                </div>
                <div className="flex items-center gap-3">
                  <button
                    onClick={() => openEdit(selected)}
                    className="text-text-secondary hover:text-text-base"
                    title={t("knowledgeView.edit")}
                  >
                    <FontAwesomeIcon icon={["fas", "pen"]} />
                  </button>
                  <button
                    onClick={() => onDelete(selected)}
                    className="text-text-secondary hover:text-red-500"
                    title={t("knowledgeView.delete")}
                  >
                    <FontAwesomeIcon icon={["fas", "trash"]} />
                  </button>
                  <button onClick={() => setSelected(null)} className="text-text-secondary hover:text-text-base">
                    <FontAwesomeIcon icon={["fas", "xmark"]} />
                  </button>
                </div>
              </div>
              <div className="flex-1 overflow-y-auto px-6 py-4">
                {selected.tags.length > 0 && (
                  <>
                    <div className="text-xs font-medium text-text-secondary uppercase tracking-wide mb-2">
                      {t("knowledgeView.fieldTags")}
                    </div>
                    <div className="flex flex-wrap gap-1.5 mb-5">
                      {selected.tags.map((tag) => (
                        <span key={tag} className="text-[11px] bg-white border border-border-theme rounded-full px-2 py-0.5 text-text-secondary">
                          #{tag}
                        </span>
                      ))}
                    </div>
                  </>
                )}
                <div className="text-xs font-medium text-text-secondary uppercase tracking-wide mb-2">
                  {t("knowledgeView.fieldBody")}
                </div>
                <pre className="text-[12px] text-text-base whitespace-pre-wrap font-mono leading-relaxed bg-white border border-border-theme rounded-lg p-3">
                  {selected.body}
                </pre>
              </div>
            </>
          ) : null}
        </div>
      )}
    </div>
  );
}
