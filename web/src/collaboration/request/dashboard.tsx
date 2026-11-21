import { MessagesSquare } from "lucide-react";
import { DutiesPanel } from "../../duties";
import { OrgChartPanel } from "../../org-chart";
import { useTenantActions } from "../../tenant-administration";
import type { State } from "../../types";
import { useI18n } from "../../ui";
import { RequestWelcome } from "./welcome";

/**
 * The dashboard without a selected Thread (#1008, ADR 0140): the Thread list,
 * the Duty list and the org chart replace the Workspace list. A person's
 * request opens a Thread whose Root Task carries it; a Duty works in its own
 * Thread.
 */
export function ThreadDashboard({
  data,
  unavailable,
  create,
  select,
}: {
  data: State;
  unavailable: boolean;
  create: () => void;
  select: (thread: string) => void;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const { can } = useTenantActions(data.access);
  const access = data.access.kind === "subject" ? data.access : undefined;
  return (
    <div className="flex h-full min-h-0 flex-col overflow-auto">
      <div className="min-h-[260px] shrink-0">
        <RequestWelcome unavailable={unavailable} create={create} />
      </div>
      <div className="grid gap-4 p-4 lg:grid-cols-3">
        <section
          aria-label={ja ? "スレッド" : "Threads"}
          className="grid content-start gap-2 rounded-lg border border-border bg-surface p-3"
        >
          <h2 className="text-sm font-semibold text-foreground">
            {ja ? "スレッド" : "Threads"}
          </h2>
          {data.threads.length === 0 ? (
            <p className="text-xs text-muted-foreground">
              {ja ? "まだスレッドはありません。" : "No threads yet."}
            </p>
          ) : (
            <ul className="grid gap-px">
              {data.threads.map((thread) => (
                <li key={thread.id}>
                  <button
                    type="button"
                    onClick={() => select(thread.id)}
                    className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[13px] text-foreground transition-colors duration-150 hover:bg-raised"
                  >
                    <MessagesSquare
                      aria-hidden
                      className="size-3.5 shrink-0 text-faint"
                    />
                    <span className="truncate font-mono">{thread.title}</span>
                  </button>
                </li>
              ))}
            </ul>
          )}
        </section>
        <section
          aria-label={ja ? "デューティ" : "Duties"}
          className="min-w-0 rounded-lg border border-border bg-surface p-3"
        >
          {access ? (
            <DutiesPanel
              key={`${data.node.id}:${access.tenant}:${access.subject}`}
              scope={{ kind: "mine" }}
              administrator={can("duty.administer")}
            />
          ) : (
            <p className="text-xs text-muted-foreground">
              {ja
                ? "デューティは個人のアカウントで表示されます。"
                : "Duties are shown to a personal account."}
            </p>
          )}
        </section>
        <section
          aria-label={ja ? "組織図" : "Org chart"}
          className="min-w-0 rounded-lg border border-border bg-surface p-3"
        >
          {access ? (
            <OrgChartPanel
              key={`org-chart-${access.tenant}`}
              tenant={access.tenant}
            />
          ) : (
            <p className="text-xs text-muted-foreground">
              {ja
                ? "組織図はテナントの中で表示されます。"
                : "The org chart is shown inside a Tenant."}
            </p>
          )}
        </section>
      </div>
    </div>
  );
}
