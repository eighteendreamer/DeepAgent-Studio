import { createContext, useContext } from "react";

/** Morph 面板是否正在 layout 形变；为 true 时药丸每帧重测以跟随行位 */
export const MorphPanelLayoutContext = createContext(false);

/** Keep a trigger's informational hint out of the way of its active menu. */
export const MenuTriggerOpenContext = createContext(false);

export function useMorphPanelLayoutAnimating() {
  return useContext(MorphPanelLayoutContext);
}
