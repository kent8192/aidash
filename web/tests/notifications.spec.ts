import { expect, test, type Page } from "@playwright/test";
import type { State } from "../src/types";
import { NOTICE_INTERVAL } from "../src/notifications/model";
import { setup } from "./collaboration-fixture";

async function stateAfterLoad(page: Page) {
  await expect(
    page.getByRole("textbox", { name: "Message", exact: true }),
  ).toBeVisible();
  const data: State = await page.evaluate(() =>
    fetch("/api/state").then((response) => response.json()),
  );
  await page.route("**/api/state**", (route) => route.fulfill({ json: data }));
  return data;
}

function addRequest(data: State, id: string, prompt: string) {
  data.human_requests.push({
    ...data.human_requests[0],
    id,
    workspace_id: "workspace-two",
    run_id: "run-1",
    kind: "APPROVAL_REQUIRED",
    prompt,
    response: null,
  });
}

// The dashboard re-reads state on window focus rather than on a short poll (#664).
async function refreshState(page: Page) {
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
}

async function nextToast(page: Page) {
  await refreshState(page);
  await page.clock.runFor(5500);
  await expect
    .poll(async () => {
      await page.clock.runFor(250);
      return page.locator(".intent-toast").count();
    })
    .toBe(1);
  await expect(page.locator(".intent-toast-open")).toBeVisible();
}

test("an older task completing emits one notice from the visible event", async ({
  page,
}) => {
  await page.clock.install();
  await setup(page, { subject: true, locale: "en-US" });
  await page.goto("/collaboration?channel=workspace-one");
  const data = await stateAfterLoad(page);
  const old = {
    ...data.tasks[0],
    id: "old-task",
    title: "Older task",
    workspace_id: "older-workspace",
    status: "COMPLETED",
    revision: 42,
  };
  data.tasks = data.tasks.filter((task) => task.id !== old.id);
  data.events.push({
    id: "completion-event",
    sequence: 123456,
    kind: "task.completed",
    workspace_id: old.workspace_id,
    node: data.workspaces[0].node,
    created_at: new Date().toISOString(),
    data: { task: old },
  });
  await nextToast(page);
  await expect(page.locator(".intent-toast-open")).toContainText("Older task");
  await refreshState(page);
  await page.clock.runFor(6000);
  await expect(page.locator(".intent-toast")).toHaveCount(0);
});

test("a notice opens an older task through its workspace snapshot", async ({
  page,
}) => {
  await page.clock.install();
  await setup(page, { subject: true, locale: "en-US" });
  await page.goto("/collaboration?channel=workspace-one");
  const data = await stateAfterLoad(page);
  const workspace = {
    ...data.workspaces[0],
    id: "older-workspace",
    title: "Older workspace",
  };
  const task = {
    ...data.tasks[0],
    id: "old-task",
    title: "Older task",
    workspace_id: workspace.id,
    status: "COMPLETED" as const,
    revision: 42,
  };
  await page.route("**/api/workspaces/older-workspace", (route) =>
    route.fulfill({
      json: {
        workspace,
        tasks: [task],
        artifacts: [],
        events: [],
        messages: [],
      },
    }),
  );
  data.events.push({
    id: "completion-event",
    sequence: 123456,
    kind: "task.completed",
    workspace_id: workspace.id,
    node: workspace.node,
    created_at: new Date().toISOString(),
    data: { task },
  });
  await nextToast(page);
  await page.locator(".intent-toast-open").click();
  await expect(page).toHaveURL(/channel=older-workspace/);
  await expect(
    page.getByRole("dialog").getByRole("heading", { name: "Older task" }),
  ).toBeVisible();
});

for (const width of [1440, 390]) {
  test(`quiet baseline and actionable approval toast at ${width}px`, async ({
    page,
  }, testInfo) => {
    await page.setViewportSize({ width, height: 1000 });
    await page.clock.install();
    const { errors, submissions } = await setup(page, {
      subject: true,
      locale: "ja-JP",
    });
    if (width === 390)
      await page.addInitScript(() =>
        localStorage.setItem("aidash-theme", "dark"),
      );
    await page.goto("/graph?channel=workspace-one&focus=old-selection");
    await expect(
      page.getByRole("combobox", { name: "グラフの視点", exact: true }),
    ).toBeVisible();
    const data: State = await page.evaluate(() =>
      fetch("/api/state").then((response) => response.json()),
    );
    await page.route("**/api/state**", (route) =>
      route.fulfill({ json: data }),
    );
    await page.clock.runFor(10_000);
    await expect(page.locator(".intent-toast")).toHaveCount(0);
    addRequest(data, "approval-new", "リリースの実行を承認してください。");
    await nextToast(page);
    await expect(page.locator(".intent-toast-open")).toContainText(
      "承認が必要です",
    );
    const bounds = await page.locator(".intent-toast").boundingBox();
    expect(bounds!.x).toBeGreaterThanOrEqual(0);
    expect(bounds!.x + bounds!.width).toBeLessThanOrEqual(width);
    await page.screenshot({
      path: testInfo.outputPath("actionable-toast.png"),
      animations: "disabled",
      fullPage: true,
    });
    const open = page.locator(".intent-toast-open");
    if (width === 1440) {
      await open.focus();
      await page.keyboard.press("Enter");
    } else await open.click();
    await expect(page).toHaveURL(/\/collaboration\?channel=workspace-two$/);
    await expect(page.getByRole("dialog")).toHaveCount(1);
    await expect(page.getByRole("dialog")).toContainText(
      "リリースの実行を承認してください。",
    );
    expect(submissions).toEqual([]);
    expect(errors).toEqual([]);
  });
}

test("bursts produce one summary and each notification opens its exact task or request", async ({
  page,
}) => {
  await page.clock.install();
  const { errors } = await setup(page, { subject: true });
  await page.goto("/collaboration?channel=workspace-two");
  const data = await stateAfterLoad(page);
  data.tasks[0] = { ...data.tasks[0], status: "FAILED", revision: 3 };
  data.tasks[1] = { ...data.tasks[1], status: "COMPLETED", revision: 3 };
  addRequest(data, "release", "Approve release?");
  addRequest(data, "publish", "Approve publishing?");
  await page.route("**/api/workspaces/workspace-one", (route) =>
    route.fulfill({
      json: {
        workspace: data.workspaces[0],
        tasks: data.tasks.filter(
          (task) => task.workspace_id === "workspace-one",
        ),
        runs: data.runs.filter((run) => run.workspace_id === "workspace-one"),
        conversations: [],
        artifacts: [],
        events: [],
      },
    }),
  );
  await nextToast(page);
  await expect(page.locator(".intent-toast-open")).toContainText("4 updates");
  await page.locator(".intent-toast-open").click();
  const center = page.locator(".intent-notifications");
  await expect(center).toBeVisible();
  await expect(center.locator(".intent-notification-row")).toHaveCount(5);
  await center
    .getByRole("button", { name: /Task failed Collect evidence/ })
    .click();
  await expect(page).toHaveURL(/channel=workspace-one/);
  await expect(page.getByRole("dialog")).toHaveCount(1);
  await expect(
    page.getByRole("dialog").getByRole("heading", { name: "Collect evidence" }),
  ).toBeVisible();
  await expect(page.getByRole("dialog")).toContainText("Failed");
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Close", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await page
    .getByRole("button", { name: "Notifications", exact: true })
    .click();
  await center.getByRole("button", { name: /Approve publishing\?/ }).click();
  await expect(page).toHaveURL(/channel=workspace-two/);
  await expect(page.getByRole("dialog")).toHaveCount(1);
  await expect(page.getByRole("dialog")).toContainText("Approve publishing?");
  expect(errors).toEqual([]);
});

test("toasts are rate limited, deduplicated and cleared with queued updates on an authority switch", async ({
  page,
}) => {
  await page.clock.install();
  const { errors } = await setup(page);
  await page.route("**/auth/session", (route) =>
    route.fulfill({
      json: {
        id: "fixture-browser-session",
        deployment_administrator: true,
        mappings: [
          { id: "fixture-mapping", tenant: "acme", subject: "alice" },
          { id: "alice", tenant: "acme", subject: "alice" },
        ],
      },
    }),
  );
  await page.goto("/collaboration?channel=workspace-one");
  const data = await stateAfterLoad(page);
  addRequest(data, "first", "First approval");
  await nextToast(page);
  await page
    .getByRole("button", { name: "Dismiss notification", exact: true })
    .click();
  await page.clock.runFor(500);
  await expect(page.locator(".intent-toast")).toHaveCount(0);
  addRequest(data, "second", "Second approval");
  await refreshState(page);
  await page.clock.runFor(15_000);
  await expect(page.locator(".intent-toast")).toHaveCount(0);
  // The held toast shows once the interval passes, then closes after 8 s, so
  // step the clock until it appears instead of jumping past it.
  for (let step = 0; step < 40; step++) {
    if (await page.locator(".intent-toast-open").count()) break;
    await page.clock.runFor(500);
  }
  await expect(page.locator(".intent-toast-open")).toContainText(
    "Second approval",
  );
  addRequest(data, "queued", "Queued approval");
  await refreshState(page);
  await page.clock.runFor(5500);
  await page.getByLabel("Account settings", { exact: true }).click();
  await page
    .getByRole("combobox", {
      name: "Choose the authority for this tab",
      exact: true,
    })
    .selectOption("mapping:acme:alice");
  await expect(
    page.getByRole("textbox", { name: "Message", exact: true }),
  ).toBeVisible();
  await page.clock.runFor(60_000);
  await expect(page.locator(".intent-toast")).toHaveCount(0);
  await page
    .getByRole("button", { name: "Notifications", exact: true })
    .click();
  await expect(page.locator(".intent-notifications")).toContainText(
    "Queued approval",
  );
  expect(errors).toEqual([]);
});

test("recent updates and throttled requests survive a temporary state outage", async ({
  page,
}) => {
  await page.clock.install();
  const { errors } = await setup(page, { subject: true });
  await page.goto("/collaboration?channel=workspace-one");
  const data = await stateAfterLoad(page);
  let unavailable = false;
  await page.route("**/api/state**", (route) =>
    unavailable
      ? route.fulfill({
          status: 503,
          json: { error: "temporary state outage" },
        })
      : route.fulfill({ json: data }),
  );
  addRequest(data, "first", "First approval");
  await nextToast(page);
  const announcedAt = await page.evaluate(() => Date.now());
  await page
    .getByRole("button", { name: "Dismiss notification", exact: true })
    .click();
  data.tasks[0] = { ...data.tasks[0], status: "COMPLETED", revision: 3 };
  addRequest(data, "queued", "Approval queued before the outage");
  await refreshState(page);
  await page.clock.runFor(5500);
  await expect(page.locator(".intent-toast")).toHaveCount(0);
  unavailable = true;
  await refreshState(page);
  await page.clock.runFor(8000);
  await expect(page.getByRole("alert")).toContainText("temporary state outage");
  unavailable = false;
  await refreshState(page);
  await page.clock.runFor(5500);
  await expect(
    page.getByRole("textbox", { name: "Message", exact: true }),
  ).toBeVisible();
  const now = await page.evaluate(() => Date.now());
  await page.clock.runFor(
    Math.max(0, announcedAt + NOTICE_INTERVAL - now) + 500,
  );
  await expect(page.locator(".intent-toast-open")).toContainText("2 updates");
  await page.locator(".intent-toast-open").click();
  const center = page.locator(".intent-notifications");
  await expect(
    center.getByRole("button", { name: /Task completed Collect evidence/ }),
  ).toBeVisible();
  await expect(
    center.getByRole("button", { name: /Approval queued before the outage/ }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});
