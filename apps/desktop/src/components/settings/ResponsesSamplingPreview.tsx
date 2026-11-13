import { useTranslation } from "react-i18next";
import type { ResponsesSamplingPreview as SamplingPreview } from "../../types";
import { Item, ItemContent, ItemDescription, ItemGroup, ItemTitle } from "../shadcn/item";

/** Values come exclusively from the Rust provider resolver, never model-name heuristics. */
export function ResponsesSamplingPreview({ rows }: { rows: SamplingPreview[] | null }) {
  const { t } = useTranslation();
  const prefix = "settings.config.responses.sampling";
  return (
    <ItemGroup aria-label={t(`${prefix}.title`)} className="mt-4 gap-2">
      <Item variant="muted" size="sm">
        <ItemContent>
          <ItemTitle>{t(`${prefix}.title`)}</ItemTitle>
          <ItemDescription>{t(`${prefix}.description`)}</ItemDescription>
        </ItemContent>
      </Item>
      {rows === null || rows.length === 0 ? (
        <Item size="sm"><ItemDescription>{t(`${prefix}.unavailable`)}</ItemDescription></Item>
      ) : rows.map(({ model, advertised_tools, sampling }) => (
        <Item key={`${model}:${advertised_tools}`} variant="outline" size="sm"
          data-sampling-model={model} data-sampling-tools={String(advertised_tools)}>
          <ItemContent>
            <ItemTitle>{model} · {t(`${prefix}.${advertised_tools ? "withTools" : "withoutTools"}`)}</ItemTitle>
            <ItemDescription>
              {t(`${prefix}.values`, {
                configured: sampling.configured_top_p ?? t(`${prefix}.notConfigured`),
                effective: sampling.effective_top_p ?? t(`${prefix}.unknown`),
              })}
            </ItemDescription>
            <ItemDescription>{t(`${prefix}.reasons.${sampling.reason}`)}</ItemDescription>
          </ItemContent>
        </Item>
      ))}
    </ItemGroup>
  );
}
