import { useEffect, useRef } from "react";
import { mountWorkingOrb } from "./workingOrbAnimation";

export function WorkingOrb({ active }: { active: boolean }) {
  const ref = useRef<SVGSVGElement>(null);
  useEffect(() => ref.current ? mountWorkingOrb(ref.current, active) : undefined, [active]);
  return <svg ref={ref} width={20} height={20} viewBox="0 0 20 20" aria-hidden="true" className="shrink-0" data-working-orb={active ? "active" : "paused"} />;
}
