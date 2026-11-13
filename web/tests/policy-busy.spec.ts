import { test, expect } from "@playwright/test";
import { setup } from "./collaboration-fixture";

test("a refused policy change says so and offers a retry once its wait has passed", async ({
  page,
}) => {
  await setup(page, { administrator: true });
  await page.addInitScript(() => {
    localStorage.setItem("aidash-locale", "en-US");
  });
  await page.route("**/api/state**", (route) =>
    route.fulfill({
      status: 503,
      json: { error: "atomic transaction visibility pending" },
    }),
  );
  let attempts = 0;
  let revision = 1;
  await page.route("**/api/authorization/acme**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path !== "/api/authorization/acme") return route.fulfill({ json: [] });
    if (route.request().method() === "POST") {
      attempts += 1;
      if (attempts === 1)
        return route.fulfill({
          status: 503,
          headers: { "retry-after": "1" },
          json: { error: "POLICY_BUSY" },
        });
      revision += 1;
    }
    return route.fulfill({
      json: {
        revision,
        bundle: {
          tenant: "acme",
          subjects: { alice: { kind: "user" } },
          groups: {},
          roles: {},
          policies: [],
        },
      },
    });
  });
  await page.goto("/");
  await page.evaluate(() => {
    history.pushState(null, "", "/authorization");
    window.dispatchEvent(new PopStateEvent("popstate"));
  });
  const authorization = page.locator(".authorization-page");
  await authorization.getByLabel("Tenant", { exact: true }).fill("acme");
  await authorization
    .getByRole("button", { name: "Open", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Edit access policy", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("button", { name: "Review" }).click();
  await dialog.getByRole("button", { name: "Apply" }).click();

  await expect(dialog.getByRole("alert")).toContainText(
    "The change was not applied",
  );
  expect(attempts).toBe(1);
  const retry = dialog.getByRole("button", { name: "Retry", exact: true });
  await expect(retry).toBeDisabled();
  await page.waitForTimeout(1500);
  // Nothing is sent again until the person asks.
  expect(attempts).toBe(1);
  await expect(retry).toBeEnabled();
  await retry.click();
  await expect(dialog).toHaveCount(0);
  expect(attempts).toBe(2);
});
