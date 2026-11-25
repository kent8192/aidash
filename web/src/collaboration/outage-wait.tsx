import { Facts, Hint } from "../components/patterns";
import type { Run } from "../types";
import { useI18n } from "../ui";

export function OutageWait({ run }: { run: Pick<Run, "control" | "state"> }) {
  const { t } = useI18n();
  const state = run.state;
  if (
    run.control !== "ACTIVE" ||
    state?.phase !== "WAITING" ||
    state.data.reason !== "outage"
  )
    return null;
  const { service, started_at, wake_at } = state.data;
  return (
    <section className="grid min-w-0 gap-1.5" role="status">
      <p className="text-xs font-medium text-foreground">
        {t("runOutageWait")}: {service}
      </p>
      <Hint>{t("runOutageWaitHint")}</Hint>
      <Facts
        items={[
          [t("runOutageWaitSince"), new Date(started_at).toLocaleString()],
          [t("runOutageWaitNextTry"), new Date(wake_at).toLocaleString()],
        ]}
      />
    </section>
  );
}
