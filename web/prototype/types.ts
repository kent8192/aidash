// PROTOTYPE ONLY. Contract every variant implements.
import type { ComponentType } from "react";

export type Screen = "request" | "observe" | "govern";

export type Variant = {
  name: string;
  Component: ComponentType<{ screen: Screen; onScreen: (screen: Screen) => void }>;
};
