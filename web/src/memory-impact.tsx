import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import type { EntityRef } from "./types";
import { apiFetch } from "./transport";
import { Badge, useI18n } from "./ui";
import { Disclosure, Hint, Notice } from "./components/patterns";
import type { Unit } from "./memory";

type AffectedRun = {
  id: string;
  task_id: string;
  phase: string;
  control: string;
  read_revision: number;
  fenced: boolean;
};
type DerivedUnit = {
  id: string;
  bank: Unit["bank"];
  revision: number;
  kind: string;
};
export type MemoryImpact = {
  units: {
    id: string;
    readable_revision: number | null;
    runs: AffectedRun[];
    derived: DerivedUnit[];
  }[];
  complete: boolean;
  limits: {
    runs_per_unit: number;
    run_scan_per_unit: number;
    derived_visits: number;
  };
  observed_at: string;
};

/** A cancelled or finished Run has no next boundary; its read is historical. */
function finished(run: AffectedRun) {
  return (
    run.control === "CANCELLED" ||
    ["COMPLETED", "FAILED", "CANCELLED"].includes(run.phase)
  );
}

/** Content-free view of the visible Runs and derived Units that depend on a Unit. */
export function MemoryImpactView({
  workspace,
  base,
  provider,
  unit,
}: {
  workspace: string;
  base: string;
  provider: EntityRef;
  unit: Unit;
}) {
  const { locale } = useI18n();
  const text = (en: string, jp: string) => (locale === "ja-JP" ? jp : en);
  const [open, setOpen] = useState(false);
  const impact = useQuery({
    // Under the page's workspace prefix, so memory changes and Refresh refetch
    // it: another Unit's change can alter dependents at the same revision.
    queryKey: [
      "memory",
      workspace,
      "impact",
      unit.bank,
      unit.id,
      unit.revision,
    ],
    queryFn: () =>
      apiFetch<MemoryImpact>(`${base}/units/impact`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          provider: { id: provider.id, version: provider.version },
          bank: unit.bank,
          units: [unit.id],
        }),
      }),
    enabled: open,
    retry: false,
  });
  const view = impact.data?.units[0];
  return (
    <Disclosure
      summary={text(
        "Dependent Runs and derived memory",
        "依存するRunと派生メモリ",
      )}
      onToggle={(event) => setOpen(event.currentTarget.open)}
    >
      <Hint>
        {text(
          "Only Runs and memory you may see are listed. This view is not a lock: every change rechecks revisions and authority when it commits.",
          "表示権限のあるRunとメモリだけを表示します。この表示はロックではなく、変更は確定時にrevisionと権限を再確認します。",
        )}
      </Hint>
      {impact.error && (
        <Notice tone="warning">
          {text(
            "Impact is unavailable with the current authority.",
            "現在の権限では影響範囲を確認できません。",
          )}
        </Notice>
      )}
      {impact.data && !impact.data.complete && (
        <Notice tone="warning">
          {text(
            "A declared bound stopped the scan; more dependents may exist.",
            "上限に達したため走査を打ち切りました。ほかにも依存先がある可能性があります。",
          )}
        </Notice>
      )}
      {view && (
        <div className="grid gap-2 text-xs">
          <p className="text-muted-foreground">
            {view.readable_revision === null
              ? text(
                  "No revision of this Unit can be read; every earlier read is fenced.",
                  "このUnitに読めるrevisionはありません。以前の読み取りはすべて無効です。",
                )
              : text(
                  `Readable revision: ${view.readable_revision}`,
                  `読み取り可能なrevision: ${view.readable_revision}`,
                )}
          </p>
          {view.runs.length === 0 && view.derived.length === 0 && (
            <p className="text-muted-foreground">
              {text("No visible dependents.", "表示できる依存先はありません。")}
            </p>
          )}
          {view.runs.map((run) => (
            <div
              key={`${run.id}:${run.read_revision}`}
              className="flex flex-wrap items-center gap-1.5"
            >
              <code className="font-mono">{run.id}</code>
              <Badge value={run.phase} />
              <Badge value={run.control} />
              <span className="text-muted-foreground">
                {finished(run)
                  ? text(
                      `read revision ${run.read_revision}; already finished, a change does not stop it`,
                      `revision ${run.read_revision} を読み取り済み。終了済みのため、変更しても停止しません`,
                    )
                  : run.fenced
                    ? text(
                        `read revision ${run.read_revision}; stops at its next boundary`,
                        `revision ${run.read_revision} を読み取り済み。次の境界で停止します`,
                      )
                    : text(
                        `reads revision ${run.read_revision}; a change stops it`,
                        `revision ${run.read_revision} を読み取り中。変更すると停止します`,
                      )}
              </span>
            </div>
          ))}
          {view.derived.map((derived) => (
            <div
              key={derived.id}
              className="flex flex-wrap items-center gap-1.5"
            >
              <code className="font-mono">{derived.id}</code>
              <Badge value={derived.kind} />
              <span className="text-muted-foreground">
                {text(
                  `derived, revision ${derived.revision}`,
                  `派生メモリ、revision ${derived.revision}`,
                )}
              </span>
            </div>
          ))}
        </div>
      )}
    </Disclosure>
  );
}
