export type ConnectionOutcome =
  | "connected"
  | "reconnected"
  | "denied"
  | "failed"
  | "closed"
  | "redirected";

export function isConnectionOutcome(
  value: unknown,
): value is ConnectionOutcome {
  return (
    typeof value === "string" &&
    ["connected", "reconnected", "denied", "failed"].includes(value)
  );
}

/** Open synchronously from the click, before asking the server for its URL. */
export function authorizePopup(
  start: () => Promise<string>,
): Promise<ConnectionOutcome> {
  const popup = window.open("", "_blank", "popup,width=620,height=760");
  return new Promise((resolve, reject) => {
    let settled = false;
    const finish = (outcome: ConnectionOutcome) => {
      if (settled) return;
      settled = true;
      cleanup();
      popup?.close();
      resolve(outcome);
    };
    const message = (event: MessageEvent) => {
      if (
        popup &&
        event.source === popup &&
        event.origin === window.location.origin &&
        event.data?.type === "aidash.connection" &&
        isConnectionOutcome(event.data.outcome)
      )
        finish(event.data.outcome);
    };
    const timer = window.setInterval(() => {
      if (popup?.closed) finish("closed");
    }, 250);
    const cleanup = () => {
      window.removeEventListener("message", message);
      window.clearInterval(timer);
    };
    window.addEventListener("message", message);
    start()
      .then((url) => {
        if (settled) return;
        if (popup) popup.location.replace(url);
        else {
          window.location.assign(url);
          finish("redirected");
        }
      })
      .catch((reason: unknown) => {
        if (settled) return;
        settled = true;
        cleanup();
        popup?.close();
        reject(reason);
      });
  });
}
