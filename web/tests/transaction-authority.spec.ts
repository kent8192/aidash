import { test, expect } from "@playwright/test";
import { setup } from "./collaboration-fixture";

// UI contract evidence; the Rust suites exercise the same authority boundaries
// against independent PostgreSQL databases and authenticated peer HTTP.
test("subject transaction controls survive a barrier and discard revoked detail", async ({
  page,
}) => {
  await setup(page, { subject: true });
  let denied = false;
  let barrier = false;
  const operatorRequests: string[] = [];
  const transaction = {
    id: "50000000-0000-0000-0000-000000000040",
    digest: "a".repeat(64),
    decision: null,
    visible: false,
    complete: false,
    last_error: null,
    manifest: {
      id: "50000000-0000-0000-0000-000000000040",
      coordinator: "aidash://home",
      isolation: "serializable",
      deadline: "2099-01-01T00:00:00Z",
      participants: [{ node_id: "aidash://home", mutations: [] }],
    },
  };
  await page.route("**/api/transactions**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/participants") || path.endsWith("/trust"))
      operatorRequests.push(path);
    if (path === "/api/transactions")
      return route.fulfill({ json: denied ? [] : [transaction] });
    return denied
      ? route.fulfill({ status: 403, json: { error: "permission revoked" } })
      : route.fulfill({ json: { transaction, participants: [], history: [] } });
  });
  await page.route("**/api/state", (route) =>
    barrier
      ? route.fulfill({
          status: 503,
          headers: { "x-aidash-transaction-pending": "1" },
          json: { error: "atomic transaction visibility pending" },
        })
      : route.fallback(),
  );
  await page.goto("/transactions");
  await expect(
    page.getByRole("heading", { name: "Transactions", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Peer transaction trust", exact: true }),
  ).toHaveCount(0);
  await page
    .getByRole("button", { name: "Create transaction", exact: true })
    .click();
  await expect(
    page.getByRole("dialog").locator("option[value=registry_register]"),
  ).toHaveCount(0);
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Close", exact: true })
    .click();
  barrier = true;
  await page.reload();
  await expect(
    page.getByRole("heading", { name: "Transactions", exact: true }),
  ).toBeVisible();
  await page.locator("button.transaction-id").click();
  await expect(page.locator(".transaction-details")).toBeVisible();
  await expect(
    page.getByRole("button", {
      name: "Abort undecided transaction",
      exact: true,
    }),
  ).toBeVisible();
  denied = true;
  await expect(page.locator(".transaction-details")).toHaveCount(0);
  await expect(page.getByRole("dialog")).toContainText(
    "Your current permissions do not allow this operation.",
  );
  await expect(
    page.getByRole("button", {
      name: "Abort undecided transaction",
      exact: true,
    }),
  ).toHaveCount(0);
  expect(operatorRequests).toEqual([]);
});

test("authority controls distinguish pending revocation while ordinary data is unavailable", async ({
  page,
}) => {
  await setup(page);
  let pending = true;
  await page.route("**/api/state", (route) =>
    route.fulfill({
      status: 503,
      json: { error: "atomic transaction visibility pending" },
    }),
  );
  await page.route("**/api/authorization/acme**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/transaction-revocations"))
      return route.fulfill({ json: pending ? ["pending-transaction"] : [] });
    if (path === "/api/authorization/acme")
      return route.fulfill({
        json: {
          revision: 1,
          bundle: {
            tenant: "acme",
            subjects: { alice: { kind: "user" } },
            groups: {},
            roles: {},
            policies: [],
          },
        },
      });
    return route.fulfill({ json: [] });
  });
  await page.goto("/");
  // Navigate within the SPA: Vite's development /auth proxy also matches the
  // legacy /authorization path, whereas the deployed router serves this page.
  await page.evaluate(() => {
    history.pushState(null, "", "/authorization");
    window.dispatchEvent(new PopStateEvent("popstate"));
  });
  await page
    .locator(".authorization-page")
    .getByLabel("Tenant", { exact: true })
    .fill("acme");
  await page
    .locator(".authorization-page")
    .getByRole("button", { name: "Open", exact: true })
    .click();
  const notice = page.getByText("New admissions are denied after revocation.", {
    exact: false,
  });
  await expect(notice).toBeVisible();
  pending = false;
  await expect(notice).toHaveCount(0);
});
