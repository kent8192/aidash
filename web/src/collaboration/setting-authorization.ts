import { settingRequestAnswer } from "../generated/aidash";
import { dashboardContext } from "../transport";

// A callback may finish a form only in the tab and account context that started it.
const pendingKey = "aidash-setting-authorization";

export function settingAuthorization(id: string, provider: boolean) {
  const nonce = crypto.randomUUID();
  const url = new URL(window.location.href);
  url.searchParams.set("setting_flow", nonce);
  url.searchParams.delete("connection");
  url.searchParams.delete("provider_authorization");
  sessionStorage.setItem(
    pendingKey,
    JSON.stringify({
      nonce,
      id,
      provider,
      context: dashboardContext(),
    }),
  );
  return url.pathname + url.search + url.hash;
}

export function finishSettingAuthorization(url: URL) {
  const nonce = url.searchParams.get("setting_flow");
  if (!nonce) return;
  url.searchParams.delete("setting_flow");
  window.history.replaceState(window.history.state, "", url);
  const stored = sessionStorage.getItem(pendingKey);
  if (!stored) return;
  const pending = JSON.parse(stored);
  if (pending.nonce !== nonce || dashboardContext() !== pending.context) return;
  sessionStorage.removeItem(pendingKey);
  const outcome = url.searchParams.get(
    pending.provider ? "provider_authorization" : "connection",
  );
  if (outcome === "denied")
    return settingRequestAnswer(pending.id, { action: "decline" });
  if (
    outcome === "connected" ||
    outcome === "reconnected" ||
    (pending.provider && outcome === "unbound")
  )
    return settingRequestAnswer(pending.id, { action: "done" });
}
