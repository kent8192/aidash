import { expect, test } from "@playwright/test";
import { setup } from "./collaboration-fixture";

// ADR 0102: a Deployment Administrator's role grants no Tenant content
// authority, so the dashboard shows none and points at a Tenant Mapping.
const notice = "Tenant content is not available to a Deployment Administrator.";

for (const path of [
  "/collaboration?channel=workspace-one",
  "/graph",
  "/creator",
  "/settings?view=providerCredentials",
  "/settings?view=registry",
]) {
  test(`the Deployment Administrator context hides Tenant content at ${path}`, async ({
    page,
  }) => {
    const { errors } = await setup(page, { administrator: true });
    const streams: string[] = [];
    page.on("request", (request) => {
      if (/\/api\/events\/stream|\/inference\/stream/.test(request.url()))
        streams.push(request.url());
    });
    await page.goto(path);
    await expect(page.getByText(notice)).toBeVisible();
    await expect(
      page.getByRole("button", { name: "Switch to a Mapping" }),
    ).toBeVisible();
    expect(streams).toEqual([]);
    expect(errors).toEqual([]);
  });
}

test("the switch control opens the authority switcher", async ({ page }) => {
  await setup(page, { administrator: true });
  await page.goto("/collaboration");
  await page.getByRole("button", { name: "Switch to a Mapping" }).click();
  await expect(page.getByRole("combobox").first()).toBeVisible();
});

test("a Mapping still shows Tenant content", async ({ page }) => {
  await setup(page, { subject: true });
  await page.goto("/collaboration?channel=workspace-one");
  await expect(page.getByText(notice)).toHaveCount(0);
});
