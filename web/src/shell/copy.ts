/** Copy for the application frame (rail, context bar, flyouts, sign-in card). */
export const shellCopy = {
  "ja-JP": {
    request: "依頼",
    creator: "Creator",
    trust: "Trust",
    newRequest: "新しい依頼",
    primaryNavigation: "メインナビゲーション",
    location: "現在地",
    switchRequest: "依頼を切り替え",
    chooseRequest: "依頼を選択",
    searchHistory: "履歴を検索",
    recentRequests: "最近の依頼",
    workspaces: "ワークスペース",
    startFirst: "最初の依頼を始めましょう。",
    noMatch: "一致する依頼がありません。",
    awaiting: (count: number) => `判断待ち ${count}`,
    decisions: "判断待ち",
    decisionQueue: "判断キュー",
    decisionCount: (count: number) => `${count} 件`,
    toLight: "ライトテーマに切り替え",
    toDark: "ダークテーマに切り替え",
    theme: "テーマ",
    themes: { dark: "ダーク", light: "ライト", system: "システム" },
    authority: "権限",
    language: "言語",
    menu: "メニュー",
    closeHistory: "履歴を閉じる",
    skip: "本文へ移動",
    general: "一般",
    ago: (minutes: number) =>
      minutes < 1
        ? "たった今"
        : minutes < 60
          ? `${minutes}分前`
          : minutes < 1440
            ? `${Math.floor(minutes / 60)}時間前`
            : `${Math.floor(minutes / 1440)}日前`,
    recovery: "整合性と復旧",
    account: "アカウント設定",
    loading: "読み込み中…",
  },
  "en-US": {
    request: "Request",
    creator: "Creator",
    trust: "Trust",
    newRequest: "New request",
    primaryNavigation: "Main navigation",
    location: "Location",
    switchRequest: "Switch request",
    chooseRequest: "Choose a request",
    searchHistory: "Search history",
    recentRequests: "Recent requests",
    workspaces: "Workspaces",
    startFirst: "Start your first request.",
    noMatch: "No matching requests.",
    awaiting: (count: number) => `Decisions ${count}`,
    decisions: "Decisions",
    decisionQueue: "Decision queue",
    decisionCount: (count: number) => `${count} pending`,
    toLight: "Switch to light theme",
    toDark: "Switch to dark theme",
    theme: "Theme",
    themes: { dark: "Dark", light: "Light", system: "System" },
    authority: "Authority",
    language: "Language",
    menu: "Menu",
    closeHistory: "Close history",
    skip: "Skip to content",
    general: "General",
    ago: (minutes: number) =>
      minutes < 1
        ? "just now"
        : minutes < 60
          ? `${minutes}m ago`
          : minutes < 1440
            ? `${Math.floor(minutes / 60)}h ago`
            : `${Math.floor(minutes / 1440)}d ago`,
    recovery: "Consistency and recovery",
    account: "Account settings",
    loading: "Loading…",
  },
} as const;
/** Browser sign-in, authority and log-out copy shared by the sign-in card and the account menu. */
export const authCopy = {
  "ja-JP": {
    signIn: "Google でサインイン",
    setup:
      "管理者が Aidash の OIDC 接続を設定してください。API の Bearer 認証は引き続き利用できます。",
    choose: "このタブで使う権限を選択してください",
    operator: "operator",
    request: "登録を申請",
    pending: "登録申請は承認待ちです。",
    rejected: "登録申請は却下されました。24 時間後に再申請できます。",
    expired: "登録申請の期限が切れました。再申請できます。",
    allDevices: "全端末からログアウト",
    currentDevice: "この端末からログアウト",
  },
  "en-US": {
    signIn: "Sign in with Google",
    setup:
      "Ask an administrator to configure OIDC for Aidash. Bearer API access remains available.",
    choose: "Choose the authority for this tab",
    operator: "operator",
    request: "Request access",
    pending: "Your registration is awaiting approval.",
    rejected: "Your request was rejected. You can try again after 24 hours.",
    expired: "Your request expired. You can submit another.",
    allDevices: "Log out on all devices",
    currentDevice: "Log out on this device",
  },
} as const;
