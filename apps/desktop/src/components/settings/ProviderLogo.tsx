import { useState } from "react";
import { cn } from "../shadcn/utils";

export function ProviderLogo({ logo, name, className }: { logo: string; name: string; className?: string }) {
  const [failed, setFailed] = useState<string | null>(null);
  const image = /^(data:image\/(png|jpeg|webp|gif);base64,|\/providers\/[a-z]+\.(svg|webp)$)/.test(logo);
  return (
    <span className={cn("flex aspect-square size-10 shrink-0 items-center justify-center overflow-hidden rounded-xl border border-border-theme bg-elevated-bg text-base font-semibold text-text-secondary", className)}>
      {image && failed !== logo
        ? <img src={logo} alt="" className="size-full object-contain p-1.5" onError={() => setFailed(logo)} />
        : <span aria-hidden>{name.trim().slice(0, 1).toUpperCase() || "+"}</span>}
    </span>
  );
}
