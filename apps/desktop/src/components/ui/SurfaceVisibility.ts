import { createContext } from "react";

// Retained pages keep their data; their portal overlays must follow page visibility.
export const SurfaceVisibility = createContext(true);
