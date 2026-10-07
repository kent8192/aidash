import { expect, test, type Page } from "@playwright/test";
import type { State, Mesh } from "../src/types";
import { NOTICE_INTERVAL } from "../src/notifications/model";
import { setup } from "./collaboration-fixture";

async function stateAfterLoad(page: Page) {
  await expect(page.locator(".collab-composer textarea")).toBeVisible();
  const data: State = await page.evaluate(() =>
    fetch("/api/state").then((response) => response.json()),
  );
  await page.route("**/api/state", (route) => route.fulfill({ json: data }));
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

async function nextToast(page: Page) {
  await page.clock.runFor(5500);
  await expect
    .poll(async () => {
      await page.clock.runFor(250);
      return page.locator(".intent-toast").count();
    })
    .toBe(1);
  await expect(page.locator(".intent-toast-open")).toBeVisible();
}

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
    await page.route("**/api/state", (route) => route.fulfill({ json: data }));
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
        operator: true,
        mappings: [{ id: "alice", tenant: "acme", subject: "alice" }],
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
  await page.clock.runFor(15_000);
  await expect(page.locator(".intent-toast")).toHaveCount(0);
  await page.clock.runFor(20_000);
  await expect(page.locator(".intent-toast-open")).toContainText(
    "Second approval",
  );
  addRequest(data, "queued", "Queued approval");
  await page.clock.runFor(5500);
  await page.getByLabel("Account settings", { exact: true }).click();
  await page
    .getByRole("combobox", {
      name: "Choose the authority for this tab",
      exact: true,
    })
    .selectOption("mapping:alice");
  await expect(page.locator(".collab-composer textarea")).toBeVisible();
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

test("remote notices preserve node identity when request and workspace IDs collide", async ({
  page,
}) => {
  await page.clock.install();
  const { errors } = await setup(page, { unpairedRemoteRequest: true });
  await page.goto("/collaboration?channel=workspace-two");
  await stateAfterLoad(page);
  const remote: Mesh = await page.evaluate(() =>
    fetch("/api/mesh").then((response) => response.json()),
  );
  await page.route("**/api/mesh", (route) => route.fulfill({ json: remote }));
  remote.nodes[0].human_requests.push({
    ...remote.nodes[0].human_requests[0],
    id: "question-one",
    prompt: "Remote node needs different evidence.",
  });
  await nextToast(page);
  await expect(page.locator(".intent-toast-open")).toContainText(
    "Remote node needs different evidence.",
  );
  await expect(page.locator(".intent-toast-open")).toContainText(
    "aidash://peer",
  );
  await expect(page.locator(".intent-toast-open")).not.toContainText(
    "Research",
  );
  await page.locator(".intent-toast-open").click();
  await expect(page).toHaveURL("/collaboration?channel=workspace-two");
  await expect(page.getByRole("dialog")).toHaveCount(1);
  await expect(page.getByRole("dialog")).toContainText(
    "Remote node needs different evidence.",
  );
  await expect(page.getByRole("dialog")).not.toContainText(
    "Which market should I examine?",
  );
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Close", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Notifications", exact: true })
    .click();
  const remoteRow = page.locator(".intent-notification-row").filter({
    hasText: "Remote node needs different evidence.",
  });
  await expect(remoteRow).toContainText("aidash://peer");
  await expect(remoteRow).not.toContainText("Research");
  await expect(
    page.locator(".intent-notification-row").filter({
      hasText: "Which market should I examine?",
    }),
  ).toContainText("Research");
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
  await page.route("**/api/state", (route) =>
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
  await page.clock.runFor(5500);
  await expect(page.locator(".intent-toast")).toHaveCount(0);
  unavailable = true;
  await page.clock.runFor(8000);
  await expect(page.getByRole("alert")).toContainText("temporary state outage");
  unavailable = false;
  await page.clock.runFor(5500);
  await expect(page.locator(".collab-composer textarea")).toBeVisible();
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
