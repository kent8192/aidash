import type { GraphKind, Relationship } from "./model";

type Copy = {
  title: string; help: string; graph: string; list: string; configuration: string; runtime: string;
  filters: string; selection: string; focus: string; expand: string; collapse: string; open: string;
  unavailable: string; missing: string; empty: string; hidden: string; hiddenEdges: string;
  entity: string; type: string; relationships: string; actions: string; zoomIn: string; zoomOut: string;
  fit: string; left: string; right: string; up: string; down: string; canvasHelp: string; snapshot: string;
  types: Record<GraphKind, string>; relations: Record<Relationship, string>;
};
export const graphCopy: Record<"en-US" | "ja-JP", Copy> = {
  "en-US": {
    title: "Agent relationship graph",
    help: "Only recorded, versioned relationships are shown. Configuration is not evidence of runtime use. Private reference documents are not loaded here.",
    graph: "Graph", list: "Relationship list", configuration: "Configuration", runtime: "Runtime activity",
    filters: "Node types (the focused agent remains visible)", selection: "Selected node", focus: "Center selection",
    expand: "Expand neighbors", collapse: "Reset exploration", open: "Open details",
    unavailable: "This agent version is not available in the current authorized view.",
    missing: "Not available in this view", empty: "No recorded relationships match these filters.",
    hidden: "Additional nodes not displayed", hiddenEdges: "Additional relationships not displayed",
    entity: "Entity", type: "Type", relationships: "Recorded relationships", actions: "Actions",
    zoomIn: "Zoom in", zoomOut: "Zoom out", fit: "Fit graph", left: "Pan left", right: "Pan right", up: "Pan up", down: "Pan down",
    canvasHelp: "Drag the background to pan, scroll to zoom, or drag a neighbor to place it. Use the controls or relationship list for keyboard access.",
    snapshot: "Runtime activity is limited to runs and tasks available in this node's authorized snapshot.",
    types: { agent: "Agent", model: "Model", tool: "Tool", skill: "Skill", cluster: "Cluster", run: "Run", task: "Task" },
    relations: { model: "configured model", tool: "configured tool", skill: "configured skill", cluster: "member of", coordinator: "configured coordinator", run: "has run", executes: "executes task" },
  },
  "ja-JP": {
    title: "エージェント関係グラフ",
    help: "記録されたバージョン付きの関係だけを表示します。設定されていることと、実行時に使われたことは別です。個人参照文書は取得しません。",
    graph: "グラフ", list: "関係の一覧", configuration: "設定上の関係", runtime: "実行時の活動",
    filters: "ノードの種類（中心のエージェントは常に表示）", selection: "選択中のノード", focus: "選択ノードを中央に",
    expand: "隣接ノードを展開", collapse: "探索をリセット", open: "詳細を開く",
    unavailable: "このエージェントのバージョンは、現在の権限付き表示では利用できません。",
    missing: "この表示では利用不可", empty: "この条件に一致する関係は記録されていません。",
    hidden: "表示を省略したノード", hiddenEdges: "表示を省略した関係",
    entity: "エンティティ", type: "種類", relationships: "記録された関係", actions: "操作",
    zoomIn: "拡大", zoomOut: "縮小", fit: "全体を表示", left: "左へ移動", right: "右へ移動", up: "上へ移動", down: "下へ移動",
    canvasHelp: "背景をドラッグすると移動、スクロールすると拡大・縮小できます。隣接ノードの位置も変更できます。キーボードでは操作ボタンか関係の一覧を利用してください。",
    snapshot: "実行時の活動は、このNodeの認可済みスナップショットに含まれるRunとTaskに限定しています。",
    types: { agent: "エージェント", model: "モデル", tool: "ツール", skill: "スキル", cluster: "クラスター", run: "実行", task: "タスク" },
    relations: { model: "設定モデル", tool: "設定ツール", skill: "設定スキル", cluster: "所属先", coordinator: "設定された調整役", run: "実行履歴", executes: "実行対象タスク" },
  },
};
export type GraphCopy = Copy;
