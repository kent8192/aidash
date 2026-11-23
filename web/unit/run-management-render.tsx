import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderToStaticMarkup } from "react-dom/server";
import { LocaleContext } from "../src/ui";
import { RemoteRunManagement } from "../src/collaboration/remote-memory";
import { OutageWait } from "../src/collaboration/outage-wait";
import type { Run } from "../src/types";

export function renderManagement(control: string, mayControl: boolean) {
  const client = new QueryClient();
  client.setQueryData(["run-management", "run"], {
    id: "run",
    phase: "THINKING",
    control,
    control_source: "tenant",
    may_control: mayControl,
    declinable_call: { call: "run:1:0", tool: "task_delegate", kind: "policy" },
  });
  return renderToStaticMarkup(
    <QueryClientProvider client={client}>
      <LocaleContext.Provider value="en-US">
        <RemoteRunManagement id="run" />
      </LocaleContext.Provider>
    </QueryClientProvider>,
  );
}

export function renderOutageWait(control: Run["control"]) {
  const run: Pick<Run, "control" | "state"> = {
    control,
    state: {
      phase: "WAITING",
      data: {
        cadence: "service",
        reason: "outage",
        resume: { phase: "READY", data: {} },
        service: "Decider",
        started_at: "2026-11-23T00:00:00Z",
        wake_at: "2026-11-23T00:05:00Z",
      },
    },
  };
  return renderToStaticMarkup(
    <LocaleContext.Provider value="en-US">
      <OutageWait run={run} />
    </LocaleContext.Provider>,
  );
}
