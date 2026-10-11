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
import { Alert, Hint, Loading } from "./components/patterns";
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
const TenantIdentityAdministration = lazy(() =>
  import("./dashboard-identity").then((m) => ({
    default: m.TenantIdentityAdministration,
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
    <Collapsible defaultOpen={initiallyOpen} className="group/tool min-w-0">
      <CollapsibleTrigger asChild>
        <Button
          variant="ghost"
          className="h-10 w-full justify-between rounded-none px-0 text-[13px] font-medium text-foreground hover:bg-transparent hover:text-brand"
        >
          <span>{title}</span>
          <ChevronDown
            aria-hidden
            className="size-4 text-muted-foreground transition-transform group-data-[state=open]/tool:rotate-180"
          />
        </Button>
      </CollapsibleTrigger>
      <CollapsibleContent className="grid min-w-0 gap-4 pb-4">
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
          <Loading>{locale === "ja-JP" ? "読み込み中…" : "Loading…"}</Loading>
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
  tenant,
  initiallyOpen,
}: {
  entries: State["registry"];
  operator: boolean;
  /** The selected Mapping's Tenant, whose Tenant Administrators manage it here. */
  tenant: string | null;
  initiallyOpen: boolean;
}) {
  const { locale, t } = useI18n();
  const loading = (
    <Loading>{locale === "ja-JP" ? "読み込み中…" : "Loading…"}</Loading>
  );
  const administratorsOnly = (
    <Hint role="status">{t("administratorsOnly")}</Hint>
  );
  return (
    <ToolSection
      title={
        locale === "ja-JP" ? "アクセス権とアカウント" : "Access and accounts"
      }
      initiallyOpen={initiallyOpen}
    >
      {operator ? (
        <Suspense fallback={loading}>
          <DashboardIdentityAdministration />
          <AuthorizationPage entries={entries} />
        </Suspense>
      ) : tenant ? (
        <Suspense fallback={loading}>
          <TenantIdentityAdministration
            key={tenant}
            tenant={tenant}
            fallback={administratorsOnly}
          />
        </Suspense>
      ) : (
        administratorsOnly
      )}
    </ToolSection>
  );
}
export function ConversationTools({
  view,
  data,
  stateError,
  nodeId,
  operator,
  workspace,
  close,
}: {
  view?: "files" | "progress";
  data?: State;
  stateError?: string;
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
        className="flex flex-col gap-0 p-0"
        closeLabel={ja ? "閉じる" : "Close"}
      >
        <SheetHeader className="border-b border-border px-4 py-3">
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
        <div className="grid min-h-0 flex-1 content-start gap-4 overflow-y-auto px-4 py-3">
          <Suspense
            fallback={<Loading>{ja ? "読み込み中…" : "Loading…"}</Loading>}
          >
            {view === "files" && workspace && (
              <RequestFiles workspace={workspace} />
            )}
            {view === "files" &&
              (operator ? (
                <Hint role="status">
                  {ja
                    ? "作業ファイルを管理するには、利用者アカウントで接続してください。"
                    : "Connect with a user account to manage working files."}
                </Hint>
              ) : (
                <WorkingFileSettings workspace={workspace} />
              ))}
            {view === "progress" && (
              <>
                {stateError && <Alert>{stateError}</Alert>}
                <TransactionsPage nodeId={nodeId} operator={operator} />
              </>
            )}
          </Suspense>
          {!data && view === "files" && (
            <Hint role="status">
              {ja
                ? "依頼の情報を読み込めません。"
                : "Request information is unavailable."}
            </Hint>
          )}
        </div>
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
    <div className="grid min-w-0 gap-4 [&>*+*]:border-t [&>*+*]:border-border [&>*+*]:pt-4">
      {query.isError ? (
        <Alert
          retry={() => void query.refetch()}
          retryLabel={locale === "ja-JP" ? "再試行" : "Retry"}
        >
          {locale === "ja-JP"
            ? "成果物を読み込めません。"
            : "Could not load results."}
        </Alert>
      ) : (
        <ArtifactList artifacts={query.data?.artifacts ?? []} />
      )}
      <ChannelFiles workspace={workspace} />
    </div>
  );
}
