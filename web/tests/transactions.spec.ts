import { test, expect } from "@playwright/test";
import {
  installBearerDashboard,
  selectDashboardLanguage,
} from "./auth-fixture";
import { randomUUID } from "node:crypto";
import { acceptanceSubject } from "./tenant-fixture";

// A participant in another Deployment takes a Node Registration with
// `atomic_transactions` at the Tenant's Node, a Deployment Key on both sides,
// and a Tenant subject to submit. None of that is in this fixture, so the
// remote participant, its partition and the trust controls are not driven here.
test("transaction dashboard aborts a stale change safely and displays committed changes", async ({
  page,
  request,
}) => {
  test.setTimeout(90000);
  const token = "acceptance-access-token";
  const headers = { authorization: `Bearer ${token}` };
  const api = async (path: string, body?: unknown) => {
    const response =
      body === undefined
        ? await request.get(path, { headers })
        : await request.post(path, { headers, data: body });
    expect(response.status(), await response.text()).toBe(200);
    return response.json();
  };
  const session = await api("/api/session");
  // Only a Tenant subject creates a Workspace (#371); the Deployment
  // Administrator submits the transaction.
  const subject = acceptanceSubject();
  const created = await request.post("/api/workspaces", {
    headers: { authorization: `Bearer ${subject.token}` },
    data: { title: "Atomic dashboard", goal: "Commit or abort together" },
  });
  expect(created.status(), await created.text()).toBe(200);
  const workspace = await created.json();
  await installBearerDashboard(page, token);
  await page.addInitScript(() => {
    localStorage.setItem("aidash-locale", "en-US");
  });
  await page.goto("/transactions");
  const manifest = (expected_revision = 0) => ({
    id: randomUUID(),
    coordinator: session.node_id,
    isolation: "serializable",
    deadline: new Date(Date.now() + 300000).toISOString(),
    participants: [
      {
        node_id: session.node_id,
        mutations: [
          {
            kind: "workspace_state",
            workspace_id: workspace.id,
            expected_revision,
            state: { result: "committed" },
          },
        ],
      },
    ],
  });
  const submit = async (value: ReturnType<typeof manifest>) => {
    await page
      .getByRole("button", { name: "Create transaction", exact: true })
      .click();
    const dialog = page.getByRole("dialog");
    const mutation = value.participants[0].mutations[0];
    await dialog
      .getByLabel("Workspace", { exact: true })
      .selectOption(workspace.id);
    await dialog
      .getByLabel("Workspace state", { exact: true })
      .fill(JSON.stringify(mutation.state));
    await dialog
      .getByLabel("Revision", { exact: true })
      .fill(String(mutation.expected_revision));
    await dialog
      .getByRole("button", { name: "Review changes", exact: true })
      .click();
    await expect(dialog).toContainText("Atomic dashboard");
    const posted = page.waitForRequest(
      (request) =>
        request.url().endsWith("/api/transactions") &&
        request.method() === "POST",
    );
    await dialog
      .getByRole("button", { name: "Submit transaction", exact: true })
      .click();
    Object.assign(value, (await posted).postDataJSON());
    await expect(page.locator(".transaction-details")).toBeVisible();
    await expect(page.locator(".transaction-details")).not.toContainText(
      value.id,
    );
  };

  await submit(manifest());
  await expect(page.locator(".transaction-details > .badge")).toHaveText(
    "Committed",
    { timeout: 30000 },
  );
  await expect(
    page.getByRole("button", {
      name: "Abort undecided transaction",
      exact: true,
    }),
  ).toHaveCount(0);
  expect(
    (await api(`/api/workspaces/${workspace.id}`)).workspace.state,
  ).toEqual({ result: "committed" });
  await page.screenshot({
    path: "../private/dashboard-transactions-desktop.png",
    fullPage: true,
  });
  await page.getByRole("button", { name: "Close", exact: true }).click();

  // The Workspace is at revision 1 now, so a change that still expects 0 aborts.
  await submit(manifest());
  await expect(page.locator(".transaction-details > .badge")).toHaveText(
    "Aborted",
    { timeout: 30000 },
  );
  expect(
    (await api(`/api/workspaces/${workspace.id}`)).workspace.revision,
  ).toBe(1);
  await page.getByRole("button", { name: "Close", exact: true }).click();
  await page
    .getByRole("dialog", { name: "Progress and transactions", exact: true })
    .getByRole("button", { name: "Close", exact: true })
    .click();
  await page.goto(`/collaboration?channel=${encodeURIComponent(workspace.id)}`);
  await selectDashboardLanguage(page, "ja-JP");
  await page.getByRole("button", { name: "状況を表示", exact: true }).click();
  await page.getByRole("button", { name: "整合性と復旧", exact: true }).click();
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(
    page.getByRole("heading", { name: "分散トランザクション", exact: true }),
  ).toBeVisible();
  await page.screenshot({
    path: "../private/dashboard-transactions-mobile.png",
    fullPage: true,
  });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
});
