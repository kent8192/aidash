import { lazy, Suspense, type ReactNode } from "react";
import { ChevronDown } from "lucide-react";
import {
  Collapsible,
  CollapsibleContent,
  CollapsibleTrigger,
} from "./components/ui/collapsible";
import { Button } from "./components/ui/button";
import {
  Sheet,
  SheetContent,
  SheetDescription,
  SheetHeader,
  SheetTitle,
} from "./components/ui/sheet";
import { useI18n } from "./ui";
import type { State } from "./types";
import { useQuery } from "@tanstack/react-query";
import { workspaceGet } from "./generated/aidash";
import { ArtifactList } from "./collaboration/channel";
import { ChannelFiles } from "./collaboration/attachments";
const WorkingFileSettings = lazy(() =>
  import("./capabilities/management").then((m) => ({
    default: m.WorkingFileSettings,
  })),
);
const TransactionsPage = lazy(() =>
  import("./transactions").then((m) => ({ default: m.TransactionsPage })),
);
const GenerationPage = lazy(() =>
  import("./generation").then((m) => ({ default: m.GenerationPage })),
);
const AuthorizationPage = lazy(() =>
  import("./authorization").then((m) => ({ default: m.AuthorizationPage })),
);
const DashboardIdentityAdministration = lazy(() =>
  import("./dashboard-identity").then((m) => ({
    default: m.DashboardIdentityAdministration,
  })),
);

export function ToolSection({
  title,
  children,
  initiallyOpen = false,
}: {
  title: string;
  children: ReactNode;
  initiallyOpen?: boolean;
}) {
  return (
    <Collapsible defaultOpen={initiallyOpen} className="intent-tool-section">
      <CollapsibleTrigger asChild>
        <Button variant="ghost" className="intent-tool-trigger">
          <span>{title}</span>
          <ChevronDown size={16} />
        </Button>
      </CollapsibleTrigger>
      <CollapsibleContent className="intent-tool-content">
        {children}
      </CollapsibleContent>
    </Collapsible>
  );
}
export function CreatorTools({
  data,
  initiallyOpen,
}: {
  data: State;
  initiallyOpen: boolean;
}) {
  const { locale } = useI18n();
  return (
    <ToolSection
      title={
        locale === "ja-JP"
          ? "エージェントの自動生成と承認"
          : "Agent generation and approval"
      }
      initiallyOpen={initiallyOpen}
    >
      <Suspense
        fallback={
          <p role="status">{locale === "ja-JP" ? "読み込み中…" : "Loading…"}</p>
        }
      >
        <GenerationPage data={data} />
      </Suspense>
    </ToolSection>
  );
}
export function TrustTools({
  entries,
  operator,
  initiallyOpen,
}: {
  entries: State["registry"];
  operator: boolean;
  initiallyOpen: boolean;
}) {
  const { locale, t } = useI18n();
  return (
    <ToolSection
      title={
        locale === "ja-JP" ? "アクセス権とアカウント" : "Access and accounts"
      }
      initiallyOpen={initiallyOpen}
    >
      {operator ? (
        <Suspense
          fallback={
            <p role="status">
              {locale === "ja-JP" ? "読み込み中…" : "Loading…"}
            </p>
          }
        >
          <DashboardIdentityAdministration />
          <AuthorizationPage entries={entries} />
        </Suspense>
      ) : (
        <p className="notice" role="status">
          {t("administratorsOnly")}
        </p>
      )}
    </ToolSection>
  );
}
export function ConversationTools({
  view,
  data,
  nodeId,
  operator,
  workspace,
  close,
}: {
  view?: "files" | "progress";
  data?: State;
  nodeId: string;
  operator: boolean;
  workspace?: string;
  close: () => void;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  return (
    <Sheet
      open={!!view}
      onOpenChange={(opened) => {
        if (!opened) close();
      }}
    >
      <SheetContent
        className="intent-tools-sheet"
        data-theme={document.documentElement.dataset.theme}
        closeLabel={ja ? "閉じる" : "Close"}
      >
        <SheetHeader>
          <SheetTitle>
            {view === "files"
              ? ja
                ? "ファイル"
                : "Files"
              : ja
                ? "進捗とトランザクション"
                : "Progress and transactions"}
          </SheetTitle>
          <SheetDescription>
            {view === "files"
              ? ja
                ? "この依頼の添付・成果物・作業ファイルを管理します。"
                : "Manage attachments, results and working files for this request."
              : ja
                ? "実行の整合性・待機・復旧状況を確認します。"
                : "Inspect execution consistency, waiting and recovery."}
          </SheetDescription>
        </SheetHeader>
        <Suspense
          fallback={<p role="status">{ja ? "読み込み中…" : "Loading…"}</p>}
        >
          {view === "files" && workspace && (
            <RequestFiles workspace={workspace} />
          )}
          {view === "files" &&
            (operator ? (
              <p role="status">
                {ja
                  ? "作業ファイルを管理するには、利用者アカウントで接続してください。"
                  : "Connect with a user account to manage working files."}
              </p>
            ) : (
              <WorkingFileSettings workspace={workspace} />
            ))}
          {view === "progress" && (
            <TransactionsPage nodeId={nodeId} operator={operator} />
          )}
        </Suspense>
        {!data && view === "files" && (
          <p role="status">
            {ja
              ? "依頼の情報を読み込めません。"
              : "Request information is unavailable."}
          </p>
        )}
      </SheetContent>
    </Sheet>
  );
}

function RequestFiles({ workspace }: { workspace: string }) {
  const { locale } = useI18n();
  const query = useQuery({
    queryKey: ["workspace", workspace],
    queryFn: ({ signal }) => workspaceGet(workspace, { signal }),
    retry: false,
  });
  return (
    <>
      {query.isError ? (
        <p role="alert">
          {locale === "ja-JP"
            ? "成果物を読み込めません。"
            : "Could not load results."}
          <Button variant="outline" onClick={() => void query.refetch()}>
            {locale === "ja-JP" ? "再試行" : "Retry"}
          </Button>
        </p>
      ) : (
        <ArtifactList artifacts={query.data?.artifacts ?? []} />
      )}
      <ChannelFiles workspace={workspace} />
    </>
  );
}
