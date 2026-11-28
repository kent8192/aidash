import { createContext, useContext } from "react";
import type { PreviewAvailability } from "../generated/models";

export interface LivePreviewSelection {
  run: string;
  livePreviewId: string;
  title?: string;
  remote?: boolean;
}

export const LivePreviewContext = createContext<{
  openLive: (selection: LivePreviewSelection) => void;
  availability: PreviewAvailability | null;
}>({
  openLive: () => undefined,
  availability: { available: false, preview_domain: null },
});

export const useLivePreview = () => useContext(LivePreviewContext);
