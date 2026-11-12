import { expect, test } from "@playwright/test";
import { setup } from "./collaboration-fixture";
import type { Unconfirmed } from "../src/unconfirmed";

// The default Node of the fixture's Tenant answers to its label; this one is
// not in the Tenant's Node list, so a warning names its address.
const known = "aidash://home/default-node";
const stranger = "aidash://elsewhere/quiet-owl-k3m9q2xa";

const entry = (
  node: string,
  kind: Unconfirmed["kind"],
  count: number,
  workspace_id = "workspace-one",
): Unconfirmed => ({ node, kind, workspace_id, count });

const english = (count: number, node: string) =>
  `${count} items are not shown because ${node} could not be reached to confirm them. They are checked again once it answers.`;
const japanese = (count: number, node: string) =>
  `${node} に確認できなかったため、${count} 件を表示していません。${node} が応答したら確認し直します。`;

test("the Task list names each silent Node once, with the Tasks it left out", async ({
  page,
}) => {
  const { errors } = await setup(page, {
    subject: true,
    unconfirmed: [
      entry(stranger, "task", 2),
      entry(known, "task", 3),
      entry(stranger, "task", 1),
      // Other kinds and other Workspaces do not belong to this list.
      entry(known, "artifact", 9),
      entry(known, "task", 40, "workspace-two"),
    ],
  });
  await page.goto("/collaboration?channel=workspace-one");
  await page.getByRole("tab", { name: /^Tasks/ }).click();
  const notice = page.getByRole("status").filter({ hasText: "not shown" });
  await expect(notice).toHaveCount(1);
  const lines = notice.locator("p");
  await expect(lines).toHaveText([
    // Ordered by address, whatever the order of the response.
    english(3, stranger),
    english(3, "Default"),
  ]);
  await expect(page.getByText("Collect evidence").first()).toBeVisible();
  expect(errors).toEqual([]);
});

test("an empty Task list with a count shows the warning instead of its empty text", async ({
  page,
}) => {
  const { errors } = await setup(page, {
    subject: true,
    withoutWork: true,
    unconfirmed: [entry(known, "task", 4)],
  });
  await page.goto("/collaboration?channel=workspace-one");
  await page.getByRole("tab", { name: /^Tasks/ }).click();
  await expect(page.getByText(english(4, "Default"))).toBeVisible();
  await expect(page.getByText("No tasks in this channel yet.")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("an empty Task list with nothing unconfirmed keeps its empty text", async ({
  page,
}) => {
  const { errors } = await setup(page, { subject: true, withoutWork: true });
  await page.goto("/collaboration?channel=workspace-one");
  await page.getByRole("tab", { name: /^Tasks/ }).click();
  await expect(page.getByText("No tasks in this channel yet.")).toBeVisible();
  await expect(page.getByText("could not be reached")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("a Workspace without the field reads as nothing left out", async ({
  page,
}) => {
  const { errors } = await setup(page, {
    subject: true,
    omitUnconfirmed: true,
    unconfirmed: [entry(known, "task", 3)],
  });
  await page.goto("/collaboration?channel=workspace-one");
  await page.getByRole("tab", { name: /^Tasks/ }).click();
  await expect(page.getByText("Collect evidence").first()).toBeVisible();
  await expect(page.getByText("could not be reached")).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("results and conversation warn about their own kinds", async ({
  page,
}) => {
  const { errors } = await setup(page, {
    subject: true,
    unconfirmed: [
      entry(known, "artifact", 2),
      entry(stranger, "message", 5),
      entry(known, "task", 7),
    ],
  });
  await page.goto("/collaboration?channel=workspace-one");
  // The conversation reads pages of messages; only message counts show there.
  await expect(page.getByText(english(5, stranger))).toBeVisible();
  await expect(page.getByText(english(2, "Default"))).toHaveCount(0);
  await page.getByRole("tab", { name: /^Results/ }).click();
  await expect(page.getByText(english(2, "Default"))).toBeVisible();
  await expect(
    page
      .locator('section[aria-label="Results"]')
      .getByText("No matching information."),
  ).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("the warning is in Japanese, with the Node named twice", async ({
  page,
}) => {
  const { errors } = await setup(page, {
    subject: true,
    locale: "ja-JP",
    unconfirmed: [entry(known, "task", 3)],
  });
  await page.goto("/collaboration?channel=workspace-one");
  await page.getByRole("tab", { name: /^タスク/ }).click();
  await expect(page.getByText(japanese(3, "Default"))).toBeVisible();
  expect(errors).toEqual([]);
});
