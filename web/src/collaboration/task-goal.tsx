import { useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { Button } from "../components/ui/button";
import { Textarea } from "../components/ui/textarea";
import type { Submit } from "../forms";
import { taskGoal, taskGoalChange } from "../generated/aidash";
import type { Task } from "../types";
import { Badge, Field, Panel, useI18n } from "../ui";

export function TaskGoal({
  task,
  editable,
  submit,
}: {
  task: Task;
  editable: boolean;
  submit: Submit;
}) {
  const { locale, t } = useI18n();
  const ja = locale === "ja-JP";
  const [draft, setDraft] = useState<string | null>(null);
  const history = useQuery({
    queryKey: ["task-goal", task.id],
    queryFn: ({ signal }) => taskGoal(task.id, { signal }),
    refetchInterval: 2000,
    retry: false,
  });
  const detail = history.data;
  const last = detail?.verdicts.at(-1);
  const resume =
    task.status === "FAILED" && last?.judgment.verdict === "impossible";
  const canEdit =
    editable &&
    (resume ||
      !["COMPLETED", "FAILED", "CANCELLED", "ABANDONED"].includes(task.status));
  const goal = detail ? detail.goal : task.goal;
  return (
    <Panel title={t("taskGoal")}>
      <div className="grid gap-3">
        <p className="whitespace-pre-wrap text-sm">
          {goal ?? (ja ? "ゴール未設定" : "No Goal set")}
        </p>
        {history.isError && (
          <p role="status" className="text-xs text-faint">
            {ja
              ? "ゴールの履歴を読み込めません。"
              : "Could not load Goal history."}
          </p>
        )}
        {canEdit && draft === null && (
          <Button variant="outline" onClick={() => setDraft(goal ?? "")}>
            {ja ? "ゴールを編集" : "Edit Goal"}
          </Button>
        )}
        {canEdit && draft !== null && (
          <form
            className="grid gap-2"
            onSubmit={(event) => {
              event.preventDefault();
              void submit(() =>
                taskGoalChange(task.id, {
                  goal: draft.trim() || null,
                  expected_revision: task.revision,
                  resume,
                }),
              ).then((saved) => {
                if (saved) {
                  setDraft(null);
                  void history.refetch();
                }
              });
            }}
          >
            <Field label={t("taskGoal")}>
              <Textarea
                value={draft}
                onChange={(event) => setDraft(event.target.value)}
                required={resume}
                maxLength={4000}
                rows={4}
              />
            </Field>
            <div className="flex gap-2">
              <Button type="submit" disabled={draft.trim() === (goal ?? "")}>
                {resume
                  ? ja
                    ? "保存して再開"
                    : "Save and resume"
                  : ja
                    ? "保存"
                    : "Save"}
              </Button>
              <Button
                type="button"
                variant="outline"
                onClick={() => setDraft(null)}
              >
                {ja ? "取消" : "Cancel"}
              </Button>
            </div>
          </form>
        )}
        {detail && detail.verdicts.length > 0 && (
          <ul
            aria-label={ja ? "判定履歴" : "Verdict history"}
            className="grid divide-y divide-border"
          >
            {[...detail.verdicts].reverse().map((verdict) => (
              <li
                key={`${verdict.run_id}:${verdict.response_epoch}:${verdict.judged_at}`}
                className="grid gap-1 py-2"
              >
                <div className="flex flex-wrap items-center gap-2 text-xs">
                  <Badge value={verdict.judgment.verdict} />
                  <time dateTime={verdict.judged_at}>
                    {new Date(verdict.judged_at).toLocaleString(locale)}
                  </time>
                  <span>
                    {verdict.model.id}@{verdict.model.version}
                  </span>
                  <span>
                    {verdict.cost_nanocredits == null
                      ? ja
                        ? "費用不明"
                        : "Cost unreported"
                      : `${(verdict.cost_nanocredits / 1_000_000_000).toLocaleString(locale, { maximumFractionDigits: 9 })} credits`}
                  </span>
                </div>
                <p className="whitespace-pre-wrap text-sm">
                  {verdict.judgment.reason}
                </p>
              </li>
            ))}
          </ul>
        )}
        {detail && detail.edits.length > 0 && (
          <details className="text-sm">
            <summary>{ja ? "編集履歴" : "Edit history"}</summary>
            <ul className="grid divide-y divide-border">
              {[...detail.edits].reverse().map((edit) => (
                <li
                  key={`${edit.edited_at}:${edit.edited_by}`}
                  className="py-2"
                >
                  <p className="text-xs text-faint">
                    {new Date(edit.edited_at).toLocaleString(locale)} ·{" "}
                    {edit.edited_by}
                  </p>
                  <p className="whitespace-pre-wrap">
                    {edit.goal ?? (ja ? "ゴールを削除" : "Goal cleared")}
                  </p>
                </li>
              ))}
            </ul>
          </details>
        )}
      </div>
    </Panel>
  );
}
