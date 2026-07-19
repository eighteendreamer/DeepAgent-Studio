import { useState } from "react";
import { FontAwesomeIcon } from "@fortawesome/react-fontawesome";
import type { IconProp } from "@fortawesome/fontawesome-svg-core";
import type { SlashPanel, SlashPanelItem } from "../types";

type StatusKey = NonNullable<SlashPanelItem["status"]>;

const STATUS_STYLE: Record<StatusKey, { icon: IconProp; className: string }> = {
  ok: { icon: ["fas", "circle-check"], className: "text-green-600" },
  error: { icon: ["fas", "circle-exclamation"], className: "text-red-500" },
  warn: { icon: ["fas", "circle-exclamation"], className: "text-amber-500" },
  info: { icon: ["fas", "circle-info"], className: "text-blue-500" },
  muted: { icon: ["fas", "minus"], className: "text-gray-400" },
};

function StatusIcon({ status }: { status?: SlashPanelItem["status"] }) {
  if (!status) return null;
  const style = STATUS_STYLE[status];
  if (!style) return null;
  return <FontAwesomeIcon icon={style.icon} className={`${style.className} text-[11px]`} />;
}

function Badges({ badges }: { badges?: string[] }) {
  if (!badges || badges.length === 0) return null;
  return (
    <>
      {badges.map((badge, i) => (
        <span
          key={i}
          className="inline-flex items-center rounded-full bg-gray-100 px-2 py-0.5 text-[11px] font-medium text-text-secondary"
        >
          {badge}
        </span>
      ))}
    </>
  );
}

function ItemRow({ item, depth }: { item: SlashPanelItem; depth: number }) {
  const hasChildren = (item.children?.length ?? 0) > 0;
  const [open, setOpen] = useState(depth === 0);
  return (
    <div>
      <div
        className="flex items-baseline gap-2 py-1"
        style={{ paddingLeft: depth * 16 }}
      >
        {hasChildren ? (
          <button
            className="text-gray-400 hover:text-text-base transition-colors"
            onClick={() => setOpen((v) => !v)}
          >
            <FontAwesomeIcon
              icon={["fas", open ? "chevron-down" : "chevron-right"]}
              className="text-[9px]"
            />
          </button>
        ) : (
          <StatusIcon status={item.status} />
        )}
        <span
          className={`text-[13px] text-text-base ${item.mono ? "font-mono" : "font-medium"}`}
        >
          {item.label}
        </span>
        {item.value && (
          <span className="min-w-0 flex-1 truncate text-[12px] text-text-secondary">
            {item.value}
          </span>
        )}
        <div className="ml-auto flex flex-shrink-0 items-center gap-1.5">
          {hasChildren && item.status && <StatusIcon status={item.status} />}
          <Badges badges={item.badges} />
        </div>
      </div>
      {hasChildren && open && (
        <div className="border-l border-border-theme" style={{ marginLeft: depth * 16 + 5 }}>
          {item.children!.map((child, i) => (
            <ItemRow key={`${child.label}-${i}`} item={child} depth={depth + 1} />
          ))}
        </div>
      )}
    </div>
  );
}

/**
 * Renders a `slash-panel` fenced block (emitted by info-summary slash commands)
 * as a grouped, status-badged, drill-down panel — the shared UI that gives
 * every slash command Claude-Code-style categorized display. Malformed JSON
 * degrades to the raw text so a bad payload never blanks the message.
 */
export function SlashPanelBlock({ content }: { content: string }) {
  let panel: SlashPanel;
  try {
    panel = JSON.parse(content) as SlashPanel;
    if (!panel || typeof panel.title !== "string" || !Array.isArray(panel.sections)) {
      throw new Error("invalid slash panel");
    }
  } catch {
    return <pre className="whitespace-pre-wrap text-[13px] text-text-secondary">{content}</pre>;
  }

  return (
    <div className="my-2 max-w-[720px] overflow-hidden rounded-xl border border-border-theme bg-white shadow-[0_1px_2px_rgb(0,0,0,0.02)]">
      <div className="border-b border-border-theme px-4 py-3">
        <div className="text-[14px] font-semibold text-text-base">{panel.title}</div>
        {panel.subtitle && (
          <div className="mt-0.5 text-[12px] text-text-secondary">{panel.subtitle}</div>
        )}
      </div>
      <div className="divide-y divide-border-theme">
        {panel.sections.map((section, si) => (
          <div key={si} className="px-4 py-2">
            {section.heading && (
              <div className="mb-1 text-[11px] font-semibold uppercase tracking-wide text-text-secondary">
                {section.heading}
              </div>
            )}
            {section.items.length === 0 ? (
              <div className="py-1 text-[12px] text-text-secondary">—</div>
            ) : (
              section.items.map((item, ii) => (
                <ItemRow key={`${item.label}-${ii}`} item={item} depth={0} />
              ))
            )}
          </div>
        ))}
      </div>
    </div>
  );
}
