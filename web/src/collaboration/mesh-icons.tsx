import {
  Bot,
  Box,
  CircleUserRound,
  Cpu,
  FileText,
  FolderKanban,
  MessageCircle,
  Network,
  Play,
  Server,
  Sparkles,
  Target,
  Wrench,
} from "lucide-react";
import type { MeshKind } from "./mesh-model";

/** Floating canvas toolbar surface (zoom/fit and view actions). */
export const floatingToolbar =
  "mesh-camera absolute right-3 top-3 z-20 flex h-9 items-center gap-0.5 rounded-md border border-border-strong bg-surface/95 px-1 shadow-overlay";

// Reuse the application's existing, consistent stroke icon library.
const meshIcons = {
  human: CircleUserRound,
  workspace: FolderKanban,
  goal: Target,
  conversation: MessageCircle,
  task: FileText,
  run: Play,
  agent: Bot,
  tool: Wrench,
  artifact: FileText,
  cluster: Network,
  remote: Server,
  model: Cpu,
  skill: Sparkles,
};
export function MeshIcon({
  kind,
  size = 14,
  className,
}: {
  kind: MeshKind;
  size?: number;
  className?: string;
}) {
  const Icon = meshIcons[kind] ?? Box;
  return (
    <Icon
      size={size}
      strokeWidth={1.6}
      aria-hidden="true"
      className={className}
    />
  );
}
