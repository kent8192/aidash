import { test, expect } from "@playwright/test";

test("deployment dashboard shows replica changes, failures and unavailable observations in both languages", async ({
  page,
}) => {
  await page.addInitScript(() => {
    sessionStorage.setItem("aidash-token", "acceptance-access-token");
    localStorage.setItem("aidash-locale", "en-US");
  });
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/deployment");
  await expect(
    page.getByText("Kubernetes observation is not configured for this node."),
  ).toBeVisible();
  let unavailable = false;
  await page.route("**/api/deployment", (route) =>
    unavailable
      ? route.fulfill({ status: 503, json: { error: "fixture unavailable" } })
      : route.fulfill({
          json: {
            enabled: true,
            namespace: "aidash",
            release: "node-a",
            deployments: [
              {
                name: "node-a-aidash-worker",
                role: "worker",
                desired: 3,
                ready: 2,
                updated: 2,
                available: 2,
                observed: false,
                conditions: [
                  {
                    kind: "Progressing",
                    status: "True",
                    reason: "ReplicaSetUpdated",
                    message: "Waiting for new Pods.",
                  },
                ],
              },
            ],
            pods: [
              {
                name: "node-a-aidash-worker-failed",
                role: "worker",
                phase: "Pending",
                ready: false,
                terminating: false,
                restarts: 3,
                conditions: [
                  {
                    kind: "aidash",
                    status: "waiting",
                    reason: "CrashLoopBackOff",
                    message: "Waiting for recovery.",
                  },
                ],
              },
            ],
            events: [
              {
                object: "node-a-aidash-worker-failed",
                kind: "Warning",
                reason: "BackOff",
                message: "Retrying container startup.",
                count: 3,
                time: "2026-09-21T00:00:00Z",
              },
            ],
          },
        }),
  );
  await expect(
    page.getByRole("heading", { name: "Replica status", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("CrashLoopBackOff", { exact: false }),
  ).toBeVisible();
  await page.screenshot({
    path: "../.ignore/dashboard-deployment-desktop.png",
    fullPage: true,
  });
  await page.getByLabel("Language", { exact: true }).selectOption("ja-JP");
  await page.setViewportSize({ width: 390, height: 844 });
  await expect(
    page.getByRole("heading", { name: "レプリカの状態", exact: true }),
  ).toBeVisible();
  await expect(page.getByText("再起動回数", { exact: false })).toBeVisible();
  await page.screenshot({
    path: "../.ignore/dashboard-deployment-mobile.png",
    fullPage: true,
  });
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  unavailable = true;
  await expect(page.getByRole("alert")).toContainText(
    "配備状態を取得できません",
    { timeout: 10000 },
  );
  await expect(
    page.getByRole("heading", { name: "レプリカの状態", exact: true }),
  ).toHaveCount(0);
  expect(errors).toEqual([]);
});
