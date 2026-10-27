import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Alert, Hint, Loading } from "../components/patterns";
import { Button } from "../components/ui/button";
import { coreLivePreview, coreLivePreviewStop } from "../generated/aidash";
import type { LivePreview } from "../generated/models";
import type { State } from "../types";
import { useI18n } from "../ui";
import { panelClass } from "./display";
import { useFilePreview } from "./file-preview-pane";
import {
  formatDuration,
  livePreviewIsActive,
  livePreviewIsViewable,
  livePreviewState,
  livePreviewStateKey,
  livePreviewStopReasonKey,
  mayStopLivePreview,
  remainingSeconds,
} from "./live-preview-model";

/**
 * The active Run's Live preview in the thread view. The API returns no
 * viewer-specific may_stop field yet, so Stop is offered only to a Deployment
 * Administrator, the viewer the dashboard can prove may cancel the Run.
 */
export function LivePreviewPanel({ run, data }: { run: string; data: State }) {
  const { locale, t } = useI18n();
  const client = useQueryClient();
  const preview = useFilePreview();
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const query = useQuery({
    queryKey: ["live-preview", run],
    queryFn: ({ signal }) => coreLivePreview(run, { signal }),
    refetchInterval: 2000,
    retry: false,
  });
  const value: LivePreview | null | undefined = query.data?.live_preview;
  const state = livePreviewState(value?.state);
  const mayStop = mayStopLivePreview(data.access.kind, state);
  const remaining = value
    ? remainingSeconds(value.expires_at, query.dataUpdatedAt)
    : 0;

  const stop = async () => {
    if (!window.confirm(t("livePreviewStopConfirm"))) return;
    setBusy(true);
    setError("");
    try {
      await coreLivePreviewStop(run);
      await client.invalidateQueries({ queryKey: ["live-preview", run] });
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason));
    } finally {
      setBusy(false);
    }
  };

  return (
    <details className={panelClass} open>
      <summary>
        {t("livePreviewTitle")}
        {value && (
          <span className="ml-1 font-normal text-muted-foreground">
            · {t(livePreviewStateKey(state))}
          </span>
        )}
      </summary>
      {query.isPending && <Loading />}
      {query.isError && (
        <Alert retry={() => void query.refetch()}>{query.error.message}</Alert>
      )}
      {query.isSuccess && !value && <Hint>{t("livePreviewNone")}</Hint>}
      {value && (
        <div className="grid min-w-0 gap-2">
          <p className="text-[13px] text-foreground">
            {t(livePreviewStateKey(state))}
            {state === "stopped" && (
              <span className="text-muted-foreground">
                {" "}
                · {t(livePreviewStopReasonKey(value.stop_reason))}
              </span>
            )}
          </p>
          {state === "stale" && <Hint>{t("livePreviewStaleHelp")}</Hint>}
          <dl className="grid gap-1 text-xs text-muted-foreground">
            <div className="flex gap-2">
              <dt>{t("livePreviewStarted")}</dt>
              <dd className="font-mono tabular">
                {new Date(value.started_at).toLocaleString(locale)}
              </dd>
            </div>
            {livePreviewIsActive(state) && (
              <div className="flex gap-2">
                <dt>{t("livePreviewRemaining")}</dt>
                <dd className="font-mono tabular">
                  {formatDuration(remaining)}
                </dd>
              </div>
            )}
            <div className="flex gap-2">
              <dt>{t("livePreviewLimitsLabel")}</dt>
              <dd className="font-mono tabular">
                {formatDuration(value.idle_seconds)} /{" "}
                {formatDuration(value.absolute_seconds)}
              </dd>
            </div>
          </dl>
          <div className="flex flex-wrap gap-2">
            {livePreviewIsViewable(state) && (
              <Button
                variant="outline"
                size="sm"
                type="button"
                onClick={() =>
                  preview.openLive({
                    run,
                    livePreviewId: value.id,
                    title: t("livePreviewTitle"),
                  })
                }
              >
                {t("livePreviewView")}
              </Button>
            )}
            {mayStop && (
              <Button
                variant="destructive"
                size="sm"
                type="button"
                disabled={busy}
                onClick={() => void stop()}
              >
                {t("livePreviewStop")}
              </Button>
            )}
          </div>
          {error && <Alert>{error}</Alert>}
        </div>
      )}
    </details>
  );
}
