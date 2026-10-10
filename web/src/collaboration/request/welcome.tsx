import { Lock, Plus } from "lucide-react";
import { Button } from "../../components/ui/button";
import { useI18n } from "../../ui";
import { collaborationCopy } from "../copy";

/** Request destination without a workspace: first-run welcome or an inaccessible explicit channel. */
export function RequestWelcome({
  unavailable,
  create,
}: {
  unavailable: boolean;
  create: () => void;
}) {
  const { locale } = useI18n();
  const copy = collaborationCopy[locale];
  const ja = locale === "ja-JP";
  return (
    <section className="canvas-grid flex h-full min-h-0 items-center justify-center overflow-auto p-6">
      <div className="grid w-full max-w-md gap-4 rounded-lg border border-border bg-surface p-6 shadow-overlay">
        {unavailable && (
          <span className="grid size-8 place-items-center rounded-md bg-warning-soft text-warning">
            <Lock aria-hidden className="size-4" />
          </span>
        )}
        <div className="grid gap-1.5">
          <h1 className="text-xl font-semibold tracking-tight text-foreground">
            {unavailable
              ? copy.unavailable
              : ja
                ? "今日は何を進めますか？"
                : "What would you like to work on?"}
          </h1>
          <p className="text-[13px] leading-relaxed text-muted-foreground">
            {unavailable
              ? copy.channelHelp
              : ja
                ? "やりたいことを伝えてください。エージェントと一緒に進められます。"
                : "Describe your goal and work through it with your agents."}
          </p>
        </div>
        <Button type="button" className="justify-self-start" onClick={create}>
          <Plus />
          {ja ? "新しい依頼" : "New request"}
        </Button>
      </div>
    </section>
  );
}
