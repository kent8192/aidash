import type {
  ApprovalKind,
  ApprovalMode,
  ApprovalModeReason,
  RuleEffect,
} from "./generated/models";
import type {
  BlockReason,
  GivenBy,
  RefusedBy,
} from "./approval-settings-model";

type ApprovalCopy = {
  title: string;
  intro: string;
  personalOnly: string;
  modeTitle: string;
  modeHelp: string;
  modes: Record<ApprovalMode, { label: string; description: string }>;
  /** Why a mode is unavailable, or not yet in effect, in plain words. `decider`
   *  names the Tenant's approval Decider, or is empty before it is known. */
  modeReason: Record<ApprovalModeReason, (decider: string) => string>;
  reviewCatalogApproval: string;
  saved: string;
  saving: string;
  rulesTitle: string;
  rulesHelp: string;
  noRules: string;
  kindLabel: string;
  kinds: Record<ApprovalKind, { label: string; help: string }>;
  targetLabel: string;
  everyTarget: string;
  everyTargetOf: (kind: string) => string;
  effectLabel: string;
  effects: Record<RuleEffect, string>;
  effectHelp: Record<RuleEffect, string>;
  addRule: string;
  deleteRule: (kind: string, target: string) => string;
  deleteAction: string;
  disabledByPolicy: string;
  disabledByPolicyHelp: string;
  targetRequired: string;
  loadFailed: string;
  retry: string;
  bypassTitle: string;
  bypassBody: string[];
  bypassConfirm: string;
  cancel: string;
  columns: { kind: string; target: string; effect: string; status: string };
  memberOpen: string;
  memberLoading: string;
  memberMode: string;
  memberBypass: (at: string | null, when: (value: string) => string) => string;
  memberReadOnly: string;
  threadSwitcher: string;
  threadDefault: (mode: string) => string;
  threadUnavailable: Record<ApprovalModeReason, string>;
  threadFailed: string;
  threadLoadFailed: string;
  alwaysAllow: string;
  alwaysAllowScope: string;
  givenByRule: string;
  givenByBypass: string;
  givenByAuto: string;
  givenByOther: (person: string, by: "rule" | "bypass") => string;
  refusedByRule: string;
  refusedByDontAsk: string;
  refusedByOther: (person: string, by: "rule" | "dont_ask") => string;
  blockedByAuto: (reason: string | null) => string;
  blockReasons: Record<BlockReason, string>;
  decider: {
    title: string;
    help: string;
    chosen: string;
    builtIn: string;
    chosenByTenant: string;
    catalogApproval: string;
    approved: string;
    notApproved: string;
    notRegistered: string;
    select: string;
    builtInOption: string;
    saving: string;
    saved: string;
    noChoices: string;
  };
};

/** What an approval card says gave an approval in advance. */
export function givenByText(
  copy: ApprovalCopy,
  given: GivenBy,
  mine: boolean,
  person: string,
): string {
  if (given.by === "auto") return copy.givenByAuto;
  if (mine) return given.by === "rule" ? copy.givenByRule : copy.givenByBypass;
  return copy.givenByOther(person, given.by);
}

/** What an approval card says refused an approval before anyone was asked. */
export function refusedByText(
  copy: ApprovalCopy,
  refused: RefusedBy,
  mine: boolean,
  person: string,
): string {
  if (refused.by === "auto")
    return copy.blockedByAuto(
      refused.reason === null ? null : copy.blockReasons[refused.reason],
    );
  if (mine)
    return refused.by === "rule" ? copy.refusedByRule : copy.refusedByDontAsk;
  return copy.refusedByOther(person, refused.by);
}

export const approvalCopy: Record<"en-US" | "ja-JP", ApprovalCopy> = {
  "en-US": {
    title: "Approvals",
    intro:
      "Decide in advance which approvals your own Runs get without waiting for you. Whatever you choose, it never reaches beyond what your Tenant's policy lets you approve, it covers only Runs that start with you, and Aidash records each approval it gives under your name.",
    personalOnly:
      "Approvals belong to a person acting in a Tenant. A Deployment Administrator has none.",
    modeTitle: "Default Approval Mode",
    modeHelp:
      "What happens when one of your Runs would wait for an approval. You can switch the mode in any Thread.",
    modes: {
      manual: {
        label: "Manual",
        description:
          "The Run waits, and the designated approver is asked to decide.",
      },
      auto: {
        label: "Auto",
        description:
          "Your Tenant's approval Decider decides. A block refuses the call and the Run continues.",
      },
      dont_ask: {
        label: "Don't ask",
        description:
          "The approval is refused, so the Run never waits. A question to you from the model is refused too.",
      },
      bypass: {
        label: "Bypass",
        description:
          "Every approval your Tenant's policy lets you give is approved without asking. Deny and ask rules still apply.",
      },
    },
    modeReason: {
      policy: () => "Your Tenant's policy does not allow this mode.",
      not_accepted: () =>
        "You accept the risk of Bypass when you first choose it as your default.",
      decider_unapproved: (decider) =>
        `Not available yet: the approval Decider your Tenant chose${decider && ` (${decider})`} has no Catalog Approval on this Node.`,
      no_decider: (decider) =>
        `Not available: the Decider your Tenant chose${decider && ` (${decider})`} is not registered as an approval Decider.`,
    },
    reviewCatalogApproval: "Review its Catalog Approval",
    saved: "Saved",
    saving: "Saving…",
    rulesTitle: "Approval Rules",
    rulesHelp:
      "An allow, ask or deny decision on one kind of approval. A deny holds in every mode. An ask asks even in Auto and Bypass. An allow is moot in Bypass.",
    noRules: "You have no Approval Rules yet.",
    kindLabel: "Kind",
    kinds: {
      tool_call: {
        label: "Tool call",
        help: "One operation of a Tenant tool, as tool:<tool id>#<operation>. A rule names the tool, never a version.",
      },
      outbound: {
        label: "Outbound request",
        help: "An origin, as https://host with no path.",
      },
      web_search: {
        label: "Web search",
        help: "A Research provider and its version, as <provider>@<version>.",
      },
      page_reading: {
        label: "Page reading",
        help: "A page host in lowercase, without scheme, port or path.",
      },
      generation: {
        label: "Generation",
        help: "A Generation policy id.",
      },
    },
    targetLabel: "Target",
    everyTarget: "Every target",
    everyTargetOf: (kind) => `Every target of this kind (${kind})`,
    effectLabel: "Effect",
    effects: { allow: "Allow", ask: "Ask", deny: "Deny" },
    effectHelp: {
      allow: "Approve without asking.",
      ask: "Always ask, in every mode.",
      deny: "Always refuse, in every mode.",
    },
    addRule: "Add rule",
    deleteRule: (kind, target) => `Delete rule: ${kind}, ${target}`,
    deleteAction: "Delete",
    disabledByPolicy: "Disabled by your Tenant's policy",
    disabledByPolicyHelp:
      "Your Tenant's policy no longer lets you approve this in advance. The rule is kept and has no effect.",
    targetRequired: "Enter a target, or choose every target of this kind.",
    loadFailed: "Your approval settings are unavailable.",
    retry: "Retry",
    bypassTitle: "Use Bypass?",
    bypassBody: [
      "Bypass approves, without asking you, every approval that your Tenant's policy lets you give for your own Runs. That includes tool calls, outbound requests, Web disclosures and generated requests.",
      "Deny and ask rules still apply, and your Tenant's policy still bounds what Bypass can approve. Aidash records each approval under your name, naming Bypass.",
      "You can switch to another mode at any time, and a change applies to Runs that are already running.",
    ],
    bypassConfirm: "I accept the risk, use Bypass",
    cancel: "Cancel",
    columns: {
      kind: "Kind",
      target: "Target",
      effect: "Effect",
      status: "Status",
    },
    memberOpen: "Approval settings",
    memberLoading: "Loading approval settings…",
    memberMode: "Default Approval Mode",
    memberBypass: (at, when) =>
      at
        ? `Accepted the risk of Bypass on ${when(at)}`
        : "Has not accepted Bypass",
    memberReadOnly:
      "Read only. Only the member changes these; you change what they may approve through the Tenant's policy.",
    threadSwitcher: "Approval Mode for this Thread",
    threadDefault: (mode) => `Default: ${mode}`,
    threadUnavailable: {
      policy: "not allowed by policy",
      not_accepted: "choose it as default first",
      decider_unapproved: "Decider not approved",
      no_decider: "no approval Decider",
    },
    threadFailed: "The Approval Mode could not be changed.",
    threadLoadFailed: "Approval Mode unavailable",
    alwaysAllow: "Always allow",
    alwaysAllowScope:
      "Allows this call now and adds an allow rule for it to your Approval Rules, so your Runs stop asking.",
    givenByRule: "Allowed by your rule",
    givenByBypass: "Allowed by Bypass",
    givenByAuto: "Allowed by Auto",
    givenByOther: (person, by) =>
      by === "rule"
        ? `Allowed by ${person}'s rule`
        : `Allowed by ${person}'s Bypass`,
    refusedByRule: "Refused by your rule",
    refusedByDontAsk: "Refused: Don't ask",
    refusedByOther: (person, by) =>
      by === "rule"
        ? `Refused by ${person}'s rule`
        : `Refused: ${person}'s Don't ask`,
    blockedByAuto: (reason) =>
      reason === null ? "Blocked by Auto" : `Blocked by Auto: ${reason}`,
    blockReasons: {
      data_exfiltration: "data sent beyond your request",
      destructive: "destroys or irreversibly changes things",
      beyond_request: "goes beyond your request",
      injected: "driven by content the Run read",
      boundary: "contradicts a boundary you stated",
      unavailable: "the Decider was unavailable",
    },
    decider: {
      title: "Approval Decider",
      help: "The Decider that judges approvals in Auto for everyone in this Tenant. Approving its version in the Catalog approves sending the Run state it reads to the hosts below.",
      chosen: "Chosen Decider",
      builtIn: "Built-in, used until the Tenant names another",
      chosenByTenant: "Chosen by this Tenant",
      catalogApproval: "Catalog Approval on this Node",
      approved: "Approved",
      notApproved: "Not approved, so Auto is unavailable",
      notRegistered:
        "The Registry holds no Decider of hook approval under this name, so Auto is unavailable.",
      select: "Approval Decider for Auto",
      builtInOption: "Built-in",
      saving: "Saving…",
      saved: "Saved",
      noChoices:
        "Register a Decider of hook approval in the Registry to choose it here.",
    },
  },
  "ja-JP": {
    title: "承認",
    intro:
      "自分の Run が、あなたを待たずに得られる承認を、あらかじめ決めます。どう選んでも、Tenant のポリシーが承認を認める範囲を超えることはなく、あなたから始まる Run だけが対象です。Aidash が与えた承認は、あなたの名前で記録されます。",
    personalOnly:
      "承認の設定は、Tenant で活動する個人のものです。Deployment Administrator にはありません。",
    modeTitle: "既定の Approval Mode",
    modeHelp:
      "自分の Run が承認を待つことになったときの動作です。モードは Thread ごとに切り替えられます。",
    modes: {
      manual: {
        label: "Manual",
        description: "Run は待機し、指定された承認者に判断を求めます。",
      },
      auto: {
        label: "Auto",
        description:
          "Tenant の承認 Decider が判断します。ブロックされた呼び出しは拒否され、Run は続行します。",
      },
      dont_ask: {
        label: "Don't ask",
        description:
          "承認は拒否されるため、Run は待機しません。モデルからあなたへの質問も拒否されます。",
      },
      bypass: {
        label: "Bypass",
        description:
          "Tenant のポリシーが認める範囲の承認を、確認せずにすべて与えます。deny と ask のルールは引き続き適用されます。",
      },
    },
    modeReason: {
      policy: () => "このモードは Tenant のポリシーで許可されていません。",
      not_accepted: () =>
        "Bypass のリスクは、初めて既定のモードとして選ぶときに受け入れます。",
      decider_unapproved: (decider) =>
        `まだ利用できません。Tenant が選んだ承認 Decider${decider && `（${decider}）`}は、この Node で Catalog Approval を受けていません。`,
      no_decider: (decider) =>
        `利用できません。Tenant が選んだ Decider${decider && `（${decider}）`}は、承認用 Decider として登録されていません。`,
    },
    reviewCatalogApproval: "Catalog Approval を確認する",
    saved: "保存しました",
    saving: "保存しています…",
    rulesTitle: "Approval Rule",
    rulesHelp:
      "ある種類の承認に対する allow・ask・deny の判断です。deny はどのモードでも守られ、ask は Auto と Bypass でも確認します。Bypass では allow に意味はありません。",
    noRules: "Approval Rule はまだありません。",
    kindLabel: "種類",
    kinds: {
      tool_call: {
        label: "ツール呼び出し",
        help: "Tenant のツールの操作を tool:<ツール ID>#<操作> で指定します。ルールが指すのはツールで、バージョンではありません。",
      },
      outbound: {
        label: "外部接続",
        help: "パスを含まない https://host 形式の origin です。",
      },
      web_search: {
        label: "Web 検索",
        help: "Research provider とそのバージョンを <provider>@<version> で指定します。",
      },
      page_reading: {
        label: "ページ読み取り",
        help: "スキーム・ポート・パスを含まない小文字のホスト名です。",
      },
      generation: {
        label: "生成",
        help: "Generation policy の ID です。",
      },
    },
    targetLabel: "対象",
    everyTarget: "すべての対象",
    everyTargetOf: (kind) => `この種類のすべての対象（${kind}）`,
    effectLabel: "効果",
    effects: { allow: "許可", ask: "確認", deny: "拒否" },
    effectHelp: {
      allow: "確認せずに承認します。",
      ask: "どのモードでも必ず確認します。",
      deny: "どのモードでも必ず拒否します。",
    },
    addRule: "ルールを追加",
    deleteRule: (kind, target) => `ルールを削除: ${kind}、${target}`,
    deleteAction: "削除",
    disabledByPolicy: "Tenant のポリシーにより無効",
    disabledByPolicyHelp:
      "Tenant のポリシーが、これをあらかじめ承認することを認めなくなりました。ルールは残りますが、効果はありません。",
    targetRequired:
      "対象を入力するか、この種類のすべての対象を選んでください。",
    loadFailed: "承認の設定を表示できません。",
    retry: "再試行",
    bypassTitle: "Bypass を使いますか？",
    bypassBody: [
      "Bypass は、自分の Run について Tenant のポリシーが認める承認を、あなたに確認せずにすべて与えます。ツール呼び出し、外部接続、Web 開示、生成リクエストが含まれます。",
      "deny と ask のルールは引き続き適用され、Bypass が承認できる範囲も Tenant のポリシーが決めます。Aidash は与えた承認を、Bypass によるものとしてあなたの名前で記録します。",
      "モードはいつでも切り替えられ、変更は実行中の Run にも反映されます。",
    ],
    bypassConfirm: "リスクを受け入れて Bypass を使う",
    cancel: "キャンセル",
    columns: {
      kind: "種類",
      target: "対象",
      effect: "効果",
      status: "状態",
    },
    memberOpen: "承認の設定",
    memberLoading: "承認の設定を読み込んでいます…",
    memberMode: "既定の Approval Mode",
    memberBypass: (at, when) =>
      at ? `Bypass のリスクを受け入れ済み（${when(at)}）` : "Bypass は未承諾",
    memberReadOnly:
      "読み取り専用です。変更できるのは本人だけで、管理者は Tenant のポリシーで承認できる範囲を決めます。",
    threadSwitcher: "この Thread の Approval Mode",
    threadDefault: (mode) => `既定: ${mode}`,
    threadUnavailable: {
      policy: "ポリシーで不許可",
      not_accepted: "先に既定として選択",
      decider_unapproved: "Decider 未承認",
      no_decider: "承認 Decider なし",
    },
    threadFailed: "Approval Mode を変更できませんでした。",
    threadLoadFailed: "Approval Mode を表示できません",
    alwaysAllow: "常に許可",
    alwaysAllowScope:
      "この呼び出しを今回許可し、同じものを許可する allow ルールを Approval Rule に追加します。以後、あなたの Run は確認しなくなります。",
    givenByRule: "あなたのルールにより許可",
    givenByBypass: "Bypass により許可",
    givenByAuto: "Auto により許可",
    givenByOther: (person, by) =>
      by === "rule"
        ? `${person} のルールにより許可`
        : `${person} の Bypass により許可`,
    refusedByRule: "あなたのルールにより拒否",
    refusedByDontAsk: "拒否: Don't ask",
    refusedByOther: (person, by) =>
      by === "rule"
        ? `${person} のルールにより拒否`
        : `拒否: ${person} の Don't ask`,
    blockedByAuto: (reason) =>
      reason === null ? "Auto がブロック" : `Auto がブロック: ${reason}`,
    blockReasons: {
      data_exfiltration: "依頼を超えてデータが送られる",
      destructive: "破壊的、または元に戻せない変更をする",
      beyond_request: "依頼の範囲を超えている",
      injected: "Run が読んだ内容に動かされている",
      boundary: "あなたが示した境界に反している",
      unavailable: "Decider が利用できなかった",
    },
    decider: {
      title: "承認 Decider",
      help: "この Tenant の全員について、Auto で承認を判断する Decider です。Catalog でそのバージョンを承認すると、Decider が読む Run の状態を下記のホストへ送ることを承認したことになります。",
      chosen: "選択中の Decider",
      builtIn: "組み込み（Tenant が別のものを指定するまで使用）",
      chosenByTenant: "この Tenant が選択",
      catalogApproval: "この Node での Catalog Approval",
      approved: "承認済み",
      notApproved: "未承認のため、Auto は利用できません",
      notRegistered:
        "この名前の hook approval の Decider が Registry にないため、Auto は利用できません。",
      select: "Auto の承認 Decider",
      builtInOption: "組み込み",
      saving: "保存しています…",
      saved: "保存しました",
      noChoices:
        "ここで選べるよう、hook が approval の Decider を Registry に登録してください。",
    },
  },
};
