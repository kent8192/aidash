import { expect, test } from "@playwright/test";
import { setup } from "./collaboration-fixture";

for (const locale of ["ja-JP", "en-US"] as const) {
  for (const width of [1440, 390]) {
    test(`intent conversation works at ${width}px in ${locale}`, async ({
      page,
    }, testInfo) => {
      const ja = locale === "ja-JP";
      await page.setViewportSize({ width, height: 1000 });
      const { errors } = await setup(page, {
        subject: true,
        locale,
        referenceLayout: true,
      });
      await page.goto("/collaboration?channel=workspace-one");
      await expect(page.locator(".collab-composer textarea")).toBeVisible();
      await expect(page.locator(".intent-inline-progress")).toBeVisible();
      await expect(page.locator(".collab-rail")).toHaveCount(0);
      await expect(page.locator(".workspace-tabbar")).toHaveCount(0);
      await page.screenshot({
        path: testInfo.outputPath("conversation.png"),
        animations: "disabled",
        fullPage: true,
      });
      if (width < 760)
        await page
          .getByRole("button", {
            name: ja ? "チャンネル" : "Channels",
            exact: true,
          })
          .click();
      await expect(
        page.getByRole("button", {
          name: ja ? "新しい依頼" : "New request",
          exact: true,
        }),
      ).toBeVisible();
      const search = page.getByRole("searchbox", {
        name: ja ? "履歴を検索" : "Search history",
      });
      await search.fill("no-such-request");
      await expect(
        page.getByText(
          ja ? "一致する依頼がありません。" : "No matching requests.",
        ),
      ).toBeVisible();
      await search.fill("");
      if (width < 760)
        await page
          .getByRole("button", {
            name: ja ? "履歴を閉じる" : "Close history",
            exact: true,
          })
          .click();
      await page
        .getByRole("button", { name: ja ? "ファイル" : "Files", exact: true })
        .click();
      await expect(page.getByRole("dialog")).toContainText(
        "release-checklist.md",
      );
      await page
        .getByRole("dialog")
        .getByRole("button", { name: ja ? "閉じる" : "Close", exact: true })
        .click();
      await expect(page.getByRole("dialog")).toHaveCount(0);
      await page
        .getByRole("button", {
          name: ja ? "タスクと成果物" : "Tasks and results",
          exact: true,
        })
        .click();
      await expect(
        page.getByRole("dialog").locator(".collab-task").first(),
      ).toBeVisible();
      await page.keyboard.press("Escape");
      await expect(page.getByRole("dialog")).toHaveCount(0);
      expect(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= window.innerWidth,
        ),
      ).toBe(true);
      await page.goto("/settings?view=registry");
      const card = page.locator(".entity-card").first();
      await expect(card).toBeVisible();
      const cardBounds = await card.boundingBox();
      const nameBounds = await card.locator("h3").boundingBox();
      expect(nameBounds!.y + nameBounds!.height).toBeLessThanOrEqual(
        cardBounds!.y + cardBounds!.height,
      );
      expect(
        await page.evaluate(
          () => document.documentElement.scrollWidth <= window.innerWidth,
        ),
      ).toBe(true);
      await page.screenshot({
        path: testInfo.outputPath("registry.png"),
        animations: "disabled",
        fullPage: true,
      });
      expect(errors).toEqual([]);
    });
  }
}

test("legacy contextual links retain channel and remove redundant settings entries", async ({
  page,
}) => {
  const { errors } = await setup(page, { subject: true });
  await page.goto("/settings?view=workingFiles&channel=workspace-two");
  await expect(page).toHaveURL(
    /\/collaboration\?channel=workspace-two&view=files$/,
  );
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.keyboard.press("Escape");
  await page.goto("/settings?view=semantic&channel=workspace-two");
  await expect(page).toHaveURL(
    /\/settings\?channel=workspace-two&focus=semantic&view=registry$/,
  );
  await expect(page.locator(".semantic-page")).toBeVisible();
  await expect(
    page.locator(".semantic-page").getByLabel("Workspace", { exact: true }),
  ).toHaveValue("workspace-two");
  for (const view of [
    "workingFiles",
    "generation",
    "authorization",
    "semantic",
    "transactions",
  ])
    await expect(
      page.locator(`.collab-settings-select option[value="${view}"]`),
    ).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("request progress keeps transactions available when ordinary data is blocked", async ({
  page,
}) => {
  await setup(page, { subject: true });
  await page.route("**/api/state", (route) =>
    route.fulfill({ status: 503, json: { error: "visibility pending" } }),
  );
  await page.route("**/api/transactions", (route) =>
    route.fulfill({ json: [] }),
  );
  await page.goto("/settings?view=transactions&channel=workspace-one");
  await expect(page).toHaveURL(
    /\/collaboration\?channel=workspace-one&view=progress$/,
  );
  await expect(
    page
      .getByRole("dialog")
      .getByRole("heading", { name: "Transactions", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("heading", { name: "Peer transaction trust", exact: true }),
  ).toHaveCount(0);
  await page.keyboard.press("Escape");
  await page
    .getByRole("button", { name: "Consistency and recovery", exact: true })
    .click();
  await expect(
    page
      .getByRole("dialog")
      .getByRole("heading", { name: "Transactions", exact: true }),
  ).toBeVisible();
});
