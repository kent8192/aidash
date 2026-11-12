import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { renderToStaticMarkup } from "react-dom/server";
import { LocaleContext } from "../src/ui";
import { RemoteRunManagement } from "../src/collaboration/remote-memory";

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
