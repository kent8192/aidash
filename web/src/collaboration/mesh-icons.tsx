import {
  Bot,
  Box,
  CircleUserRound,
  Cpu,
  FileText,
  FolderKanban,
  MessageCircle,
  Network,
  Server,
  Sparkles,
  Target,
  Wrench,
} from "lucide-react";
import type { MeshKind } from "./mesh-model";

// Reuse the application's existing, consistent stroke icon library.
export const meshIcons = {
  human: CircleUserRound,
  workspace: FolderKanban,
  goal: Target,
  conversation: MessageCircle,
  task: FileText,
  agent: Bot,
  tool: Wrench,
  artifact: FileText,
  cluster: Network,
  remote: Server,
  model: Cpu,
  skill: Sparkles,
};
export const meshColors: Record<MeshKind, string> = {
  human: "#f4a16e",
  workspace: "#83b9ea",
  goal: "#63e9b4",
  conversation: "#6fbef1",
  task: "#6ecde9",
  agent: "#c79aee",
  tool: "#76b7e8",
  artifact: "#e2bc65",
  cluster: "#6bbda5",
  remote: "#9eaaf4",
  model: "#a2b2c8",
  skill: "#dba4cc",
};
export function MeshIcon({
  kind,
  size = 18,
}: {
  kind: MeshKind;
  size?: number;
}) {
  const Icon = meshIcons[kind] ?? Box;
  return <Icon size={size} strokeWidth={1.6} aria-hidden="true" />;
}
