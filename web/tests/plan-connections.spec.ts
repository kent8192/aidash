import { expect, test, type Page } from "@playwright/test";
import { setup as collaborationSetup } from "./collaboration-fixture";
import { installBearerDashboard } from "./auth-fixture";

const callback =
  "http://127.0.0.1:1457/auth/callback?code=private-code&state=private-state";
const metadata = {
  id: "connection",
  tenant: "alpha",
  subject: "alice",
  provider: "chatgpt",
  state: "active",
  revision: 4,
  connected_at: "2026-10-23T00:00:00Z",
  refreshed_at: null,
  refresh_expires_at: "2026-11-23T00:00:00Z",
};
async function setup(
  page: Page,
  options: {
    enabled?: boolean;
    state?: string;
    connected?: boolean;
    admin?: boolean;
    holdComplete?: Promise<void>;
    reject?: boolean;
    switchable?: boolean;
    desktop?: boolean;
    sameSubject?: boolean;
  } = {},
) {
  await installBearerDashboard(
    page,
    "fixture",
    options.admin ? undefined : { tenant: "alpha", name: "alice" },
  );
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  if (options.desktop)
    await page.addInitScript(() => {
      const state = window as unknown as {
        __TAURI_INTERNALS__: {
          invoke: (
            command: string,
            args?: Record<string, unknown>,
          ) => Promise<unknown>;
        };
        nativeCalls: { command: string; args?: Record<string, unknown> }[];
        nativeCallback: (value: string) => void;
      };
      state.nativeCalls = [];
      state.__TAURI_INTERNALS__ = {
        invoke: async (command, args) => {
          state.nativeCalls.push({ command, args });
          if (command === "connection_settings")
            return {
              profiles: [
                { id: "local", name: "Local", origin: location.origin },
              ],
              selected: "local",
            };
          if (command === "desktop_access")
            return { access_token: "native-fixture", expires_in: 300 };
          if (command === "plan_listen")
            return { id: "native-listener", port: 49123 };
          if (command === "plan_wait")
            return new Promise<string>((resolve) => {
              state.nativeCallback = resolve;
            });
          if (command === "plan_cancel") return;
          throw new Error("Unexpected native command");
        },
      };
    });
  if (options.switchable)
    await page.route("**/auth/session", (route) =>
      route.fulfill({
        json: {
          id: "fixture-browser-session",
          deployment_administrator: false,
          mappings: [
            { id: "fixture-mapping", tenant: "alpha", subject: "alice" },
            {
              id: "beta-mapping",
              tenant: options.sameSubject ? "alpha" : "beta",
              subject: options.sameSubject ? "alice" : "bob",
            },
          ],
        },
      }),
    );
  const initialAccess = options.admin
    ? { kind: "deployment_administrator" }
    : { kind: "subject", tenant: "alpha", subject: "alice" };
  let connection: Record<string, unknown> | null = options.connected
    ? { ...metadata, state: options.state ?? "active" }
    : null;
  const requests: { path: string; body: unknown }[] = [];
  await page.route("**/api/**", async (route) => {
    const req = route.request(),
      path = new URL(req.url()).pathname;
    if (
      options.sameSubject &&
      req.headers()["x-aidash-context"] === "mapping:alpha:beta-mapping" &&
      path.endsWith("/plan-connections/chatgpt")
    )
      return route.fulfill({ json: { enabled: true, connection: null } });
    const beta =
      req.headers()["x-aidash-context"] === "mapping:beta:beta-mapping";
    const access = beta
      ? { kind: "subject", tenant: "beta", subject: "bob" }
      : initialAccess;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/session")
      return route.fulfill({ json: { access, node_id: "aidash://test" } });
    if (path === "/api/state")
      return route.fulfill({
        json: {
          access,
          deployment: {
            id: "aidash://test",
            endpoint: "http://localhost",
            protocol_version: "0.1",
            key: null,
          },
          registry: [],
          workspaces: [],
          tasks: [],
          artifacts: [],
          events: [],
          runs: [],
          conversations: [],
          human_requests: [],
          installations: [],
        },
      });
    if (path.startsWith("/api/tenants/beta/plan-connections/chatgpt"))
      return route.fulfill({ json: { enabled: true, connection: null } });
    if (path.startsWith("/api/tenants/alpha/plan-connections/chatgpt")) {
      requests.push({ path, body: req.postData() ? req.postDataJSON() : null });
      if (path.endsWith("/authorizations/complete")) {
        await options.holdComplete;
        if (options.reject)
          return route.fulfill({ status: 400, json: { error: callback } });
        connection = metadata;
        return route.fulfill({ json: { outcome: "connected", connection } });
      }
      if (path.endsWith("/authorizations"))
        return route.fulfill({
          json: {
            authorization_url:
              "https://auth.openai.com/api/accounts/authorize?state=private-state",
            redirect_uri: "http://127.0.0.1:1457/auth/callback",
          },
        });
      if (req.method() === "DELETE") {
        connection = null;
        return route.fulfill({ json: { revoked: false } });
      }
      return route.fulfill({
        json: { enabled: options.enabled ?? true, connection },
      });
    }
    return route.fulfill({ json: [] });
  });
  await page.goto("/settings?view=planConnections");
  if (options.desktop)
    await page
      .getByRole("button", { name: "alpha / alice", exact: true })
      .click();
  return requests;
}

test("personal connect pastes a single-use callback, clears it and disconnects by revision", async ({
  page,
}) => {
  const requests = await setup(page);
  await expect(
    page.getByText("Whoever operates this host can read the Plan Tokens", {
      exact: false,
    }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Connect ChatGPT", exact: true })
    .click();
  await expect(
    page.getByRole("link", { name: "Continue in browser" }),
  ).toHaveAttribute("href", /^https:\/\/auth.openai.com\//);
  await page.getByLabel("Callback URL", { exact: true }).fill(callback);
  await page
    .getByRole("button", { name: "Complete connection", exact: true })
    .click();
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  expect(
    requests.find((r) => r.path.endsWith("/authorizations/complete"))?.body,
  ).toEqual({ callback_url: callback });
  expect(
    await page.evaluate(() =>
      JSON.stringify({
        local: { ...localStorage },
        session: { ...sessionStorage },
        html: document.body.innerText,
        url: location.href,
      }),
    ),
  ).not.toContain("private-code");
  await page.getByRole("button", { name: "Disconnect", exact: true }).click();
  await expect(
    page.getByText(
      "Local connection deleted. Provider revocation could not be confirmed.",
    ),
  ).toBeVisible();
  await expect(
    page.getByText("Remove the connection in ChatGPT settings as well.", {
      exact: false,
    }),
  ).toBeVisible();
  expect(
    requests.find((r) => r.path.endsWith("/chatgpt") && r.body)?.body,
  ).toEqual({ expected_revision: 4 });
});

test("configuration disablement keeps personal status and disconnect without connect", async ({
  page,
}) => {
  await setup(page, { connected: true, enabled: false });
  await expect(
    page.getByText("Plan usage is disabled on this deployment.").first(),
  ).toBeVisible();
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Reconnect", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("button", { name: "Disconnect", exact: true }),
  ).toBeEnabled();
});

test("reconnection and ineligibility have distinct persistent guidance", async ({
  page,
}) => {
  await setup(page, { connected: true, state: "ineligible" });
  await expect(
    page
      .getByText(
        "Your ChatGPT plan, workspace, or current Aidash identity/Mapping is not eligible. Check your plan and current identity/Mapping.",
      )
      .first(),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Reconnect", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("link", { name: "Manage usage" }).first(),
  ).toHaveAttribute("href", "https://chatgpt.com/settings/usage");
  await page.goto("/collaboration");
  await expect(
    page.getByText(
      "Your ChatGPT plan, workspace, or current Aidash identity/Mapping is not eligible. Check your plan and current identity/Mapping.",
    ),
  ).toBeVisible();
  await expect(
    page.getByRole("link", { name: "Open Plan Connections", exact: true }),
  ).toBeVisible();
});

test("expired grants ask for reconnection without hiding disconnect", async ({
  page,
}) => {
  await setup(page, { connected: true, state: "needs_reconnection" });
  await expect(
    page
      .getByText(
        "Your ChatGPT plan needs reconnection. Reconnect in personal settings.",
      )
      .first(),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Reconnect", exact: true }),
  ).toBeEnabled();
  await expect(
    page.getByRole("button", { name: "Disconnect", exact: true }),
  ).toBeEnabled();
});

test("cancelled completion cannot publish its late result", async ({
  page,
}) => {
  let release = () => {};
  const holdComplete = new Promise<void>((resolve) => {
    release = resolve;
  });
  await setup(page, { holdComplete });
  await page
    .getByRole("button", { name: "Connect ChatGPT", exact: true })
    .click();
  await page.getByLabel("Callback URL", { exact: true }).fill(callback);
  await page
    .getByRole("button", { name: "Complete connection", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Cancel connection", exact: true })
    .click();
  release();
  await expect(
    page.getByRole("button", { name: "Connect ChatGPT", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("Connected", { exact: true })).toHaveCount(0);
});

test("errors never reflect a callback URL into the presentation", async ({
  page,
}) => {
  await setup(page, { reject: true });
  await page
    .getByRole("button", { name: "Connect ChatGPT", exact: true })
    .click();
  await page.getByLabel("Callback URL", { exact: true }).fill(callback);
  await page
    .getByRole("button", { name: "Complete connection", exact: true })
    .click();
  await expect(page.getByRole("alert")).toContainText(
    "Could not complete the connection",
  );
  await expect(page.getByLabel("Callback URL", { exact: true })).toHaveValue(
    "",
  );
  await expect(page.locator("body")).not.toContainText("private-code");
});

test("Deployment Administrators have no personal Plan execution settings", async ({
  page,
}) => {
  const requests = await setup(page, { admin: true });
  await expect(
    page.getByText("Plan Connections belong to a signed-in subject."),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "Connect ChatGPT", exact: true }),
  ).toHaveCount(0);
  expect(requests).toEqual([]);
});

for (const sameSubject of [false, true])
  test(`switching ${sameSubject ? "Mapping for the same subject" : "Tenant and subject"} discards a pending authorization and late completion`, async ({
    page,
  }) => {
    let release = () => {};
    const holdComplete = new Promise<void>((resolve) => {
      release = resolve;
    });
    const requests = await setup(page, {
      switchable: true,
      holdComplete,
      sameSubject,
    });
    await page
      .getByRole("button", { name: "Connect ChatGPT", exact: true })
      .click();
    await page.getByLabel("Callback URL", { exact: true }).fill(callback);
    await page
      .getByRole("button", { name: "Complete connection", exact: true })
      .click();
    await page
      .getByRole("button", { name: "Account settings", exact: true })
      .click();
    await page
      .getByRole("combobox", { name: "Choose the authority for this tab" })
      .selectOption(`mapping:${sameSubject ? "alpha" : "beta"}:beta-mapping`);
    release();
    await expect(
      page.getByRole("button", { name: "Connect ChatGPT", exact: true }),
    ).toBeVisible();
    await expect(page.getByLabel("Callback URL", { exact: true })).toHaveCount(
      0,
    );
    await expect(page.getByText("Connected", { exact: true })).toHaveCount(0);
    expect(
      requests.filter((request) =>
        request.path.endsWith("/authorizations/complete"),
      ),
    ).toHaveLength(1);
  });

test("Desktop relays through its authenticated session and drops a callback after cancellation", async ({
  page,
}) => {
  const requests = await setup(page, { desktop: true });
  await page
    .getByRole("button", { name: "Connect ChatGPT", exact: true })
    .click();
  await expect(
    page.getByText("Waiting for browser authorization…"),
  ).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as unknown as { nativeCalls: { command: string }[] }
          ).nativeCalls.filter((call) => call.command === "plan_wait").length,
      ),
    )
    .toBe(1);
  expect(
    requests.find((request) => request.path.endsWith("/authorizations"))?.body,
  ).toEqual({ port: 49123 });
  await page
    .getByRole("button", { name: "Cancel connection", exact: true })
    .click();
  await page.evaluate(
    (value) =>
      (
        window as unknown as { nativeCallback: (value: string) => void }
      ).nativeCallback(value),
    callback.replace(":1457", ":49123"),
  );
  await expect(
    page.getByRole("button", { name: "Connect ChatGPT", exact: true }),
  ).toBeEnabled();
  expect(
    requests.filter((request) =>
      request.path.endsWith("/authorizations/complete"),
    ),
  ).toEqual([]);
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as unknown as {
              nativeCalls: {
                command: string;
                args?: Record<string, unknown>;
              }[];
            }
          ).nativeCalls,
      ),
    )
    .toContainEqual({
      command: "plan_cancel",
      args: { id: "native-listener" },
    });
  await page
    .getByRole("button", { name: "Connect ChatGPT", exact: true })
    .click();
  await expect(
    page.getByText("Waiting for browser authorization…"),
  ).toBeVisible();
  await expect
    .poll(() =>
      page.evaluate(
        () =>
          (
            window as unknown as { nativeCalls: { command: string }[] }
          ).nativeCalls.filter((call) => call.command === "plan_wait").length,
      ),
    )
    .toBe(2);
  await page.evaluate(
    (value) =>
      (
        window as unknown as { nativeCallback: (value: string) => void }
      ).nativeCallback(value),
    callback.replace(":1457", ":49123"),
  );
  await expect(page.getByText("Connected", { exact: true })).toBeVisible();
  expect(
    requests.filter((request) =>
      request.path.endsWith("/authorizations/complete"),
    ),
  ).toHaveLength(1);
  await expect(page.getByLabel("Callback URL", { exact: true })).toHaveCount(0);
});

test("a Plan Run explains its source and links to personal usage management", async ({
  page,
}) => {
  const { errors } = await collaborationSetup(page, {
    subject: true,
    planModel: true,
  });
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Execution history", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByRole("button")
    .filter({ hasText: "Researcher" })
    .click();
  const dialog = page.getByRole("dialog");
  await expect(
    dialog.getByText("Using ChatGPT plan", { exact: true }),
  ).toBeVisible();
  await expect(
    dialog.getByRole("link", { name: "Manage usage", exact: true }),
  ).toHaveAttribute("href", "https://chatgpt.com/settings/usage");
  await expect(dialog).toContainText(
    "Failures and usage limits stop the Run without falling back to other credentials.",
  );
  expect(errors).toEqual([]);
});
