import { expect, test } from "@playwright/test";
import { setup } from "./collaboration-fixture";

test("collaboration is the landing view, with two primary destinations and secondary settings", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "# Research", exact: true }),
  ).toBeVisible();
  const primary = page.locator(".collab-rail nav").first();
  await expect(primary.getByRole("link")).toHaveCount(2);
  await expect(
    primary.getByRole("link", { name: "Collaboration", exact: true }),
  ).toBeVisible();
  await expect(
    primary.getByRole("link", { name: "Graph View", exact: true }),
  ).toBeVisible();
  await expect(
    page.locator(".collab-secondary").getByRole("link", { name: "Settings" }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("failed message keeps its draft and idempotency key on retry", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, { failFirstMessage: true });
  await page.goto("/collaboration?channel=workspace-one");
  const composer = page.getByRole("textbox", { name: "Message", exact: true });
  await composer.fill("Please check the source.");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("Your text has been kept");
  await expect(composer).toHaveValue("Please check the source.");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(composer).toHaveValue("");
  await expect(
    page.getByText("Please check the source.", { exact: true }),
  ).toBeVisible();
  const messages = submissions.filter((item) =>
    item.path.endsWith("/thread-messages"),
  );
  expect(messages).toHaveLength(2);
  expect(messages[0].body.idempotency_key).toBe(messages[1].body.idempotency_key);
  expect(errors).toEqual([]);
});

test("preparing a channel does not invoke agent execution", async ({ page }) => {
  const { submissions, errors } = await setup(page);
  await page.goto("/");
  await page.getByRole("button", { name: "Prepare a channel", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("textbox", { name: "Title", exact: true }).fill("Draft research");
  await dialog.getByRole("textbox", { name: "Goal", exact: true }).fill("Discuss sources before starting");
  await dialog.getByRole("button", { name: "Create", exact: true }).click();
  await expect(page).toHaveURL(/channel=prepared/);
  await expect(
    page.getByRole("heading", { name: "# Draft research", exact: true }),
  ).toBeVisible();
  expect(submissions.map((item) => item.path)).toEqual(["/api/workspaces"]);
  expect(errors).toEqual([]);
});

test("graph renders with Cytoscape and offers current related channel first", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.goto("/collaboration?channel=workspace-two");
  await page.getByRole("button", { name: "View in graph", exact: true }).click();
  await expect(page).toHaveURL(/\/graph\?channel=workspace-two/);
  await expect(page.locator(".collab-cytoscape canvas").first()).toBeVisible();
  const related = page.locator(".collab-selection .collab-channel-link");
  await expect(related).toHaveCount(2);
  await expect(related.first()).toHaveText("# Planning");
  await related.filter({ hasText: "Research" }).click();
  await expect(page).toHaveURL(/\/collaboration\?channel=workspace-one/);
  await expect(
    page.getByText("Evidence is ready to review.", { exact: true }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("channel intervention answers a human request without settings navigation", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page);
  await page.goto("/");
  await page.getByRole("button").filter({ hasText: "Which market should I examine?" }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByRole("textbox", { name: "Answer", exact: true }).fill("Japan");
  await dialog.getByRole("button", { name: "Send", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(submissions.some((item) => item.path === "/api/human-requests/question-one/answer")).toBe(true);
  expect(errors).toEqual([]);
});

test("revoked history is hidden rather than retaining a stale conversation", async ({
  page,
}) => {
  const { revokeHistory } = await setup(page);
  await page.goto("/collaboration?channel=workspace-one");
  await expect(page.getByText("Evidence is ready to review.", { exact: true })).toBeVisible();
  revokeHistory();
  await expect(page.getByText("Evidence is ready to review.", { exact: true })).toHaveCount(0);
  await expect(page.getByRole("alert")).toContainText("unavailable");
});

test("an inaccessible explicit channel is not silently replaced with another channel", async ({
  page,
}) => {
  await setup(page);
  await page.goto("/collaboration?channel=unavailable");
  await expect(page.getByRole("heading", { name: "# Research", exact: true })).toHaveCount(0);
  await expect(page.getByRole("heading", {
    name: "This resource is unavailable under your current access.",
    exact: true,
  })).toBeVisible();
});

test("legacy management links are routed into secondary settings", async ({ page }) => {
  const { errors } = await setup(page);
  await page.goto("/agents");
  await expect(page).toHaveURL(/\/settings\?.*view=agents/);
  await expect(page.getByRole("heading", { name: "Researcher", exact: true })).toBeVisible();
  expect(errors).toEqual([]);
});

test("mobile conversation and channel switching do not require horizontal scrolling", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/collaboration?channel=workspace-one");
  await expect(page.getByRole("textbox", { name: "Message", exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Channels", exact: true }).click();
  await page.getByRole("link", { name: "# Planning", exact: true }).click();
  await expect(page.getByRole("heading", { name: "# Planning", exact: true })).toBeVisible();
  expect(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth)).toBe(true);
  expect(errors).toEqual([]);
});

test("thread replies persist separately and channel drafts survive thread navigation", async ({ page }) => {
  const { submissions, errors } = await setup(page);
  await page.goto("/collaboration?channel=workspace-one");
  const composer = page.getByRole("textbox", { name: "Message", exact: true });
  await composer.fill("Unsent channel note");
  await page.getByRole("button", { name: "Reply in thread", exact: true }).click();
  await expect(page.getByRole("heading", { name: "Thread", exact: true })).toBeVisible();
  await expect(composer).toHaveValue("");
  await composer.fill("Here is the supporting source.");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(page.getByText("Here is the supporting source.", { exact: true })).toBeVisible();
  await page.getByRole("button", { name: "Back to channel", exact: true }).click();
  await expect(composer).toHaveValue("Unsent channel note");
  await expect(page.getByText("Here is the supporting source.", { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Open thread", exact: true }).click();
  await expect(page.getByText("Here is the supporting source.", { exact: true })).toBeVisible();
  const posts = submissions.filter((item) => item.path.endsWith("/thread-messages"));
  expect(posts).toHaveLength(1);
  expect(posts[0].body.thread_id).toBe("thread-message-one");
  expect(errors).toEqual([]);
});

test("older message pages are prepended without losing the latest messages", async ({ page }) => {
  const { errors } = await setup(page, { olderMessages: 45 });
  await page.goto("/collaboration?channel=workspace-one");
  await expect(page.locator(".collab-message")).toHaveCount(40);
  await expect(page.getByText("Previous message 0", { exact: true })).toHaveCount(0);
  await page.getByRole("button", { name: "Load older messages", exact: true }).click();
  await expect(page.locator(".collab-message")).toHaveCount(46);
  await expect(page.getByText("Previous message 0", { exact: true })).toBeVisible();
  await expect(page.getByText("Evidence is ready to review.", { exact: true })).toHaveCount(1);
  await expect(page.getByRole("button", { name: "Load older messages", exact: true })).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("revoking a thread read hides cached replies without exposing another conversation", async ({ page }) => {
  const { revokeThreads } = await setup(page);
  await page.goto("/collaboration?channel=workspace-one");
  await page.getByRole("button", { name: "Reply in thread", exact: true }).click();
  await expect(page.getByText("Evidence is ready to review.", { exact: true })).toBeVisible();
  revokeThreads();
  await expect(page.getByRole("alert")).toContainText("unavailable");
  await expect(page.locator(".collab-message")).toHaveCount(0);
  await expect(page.getByRole("textbox", { name: "Message", exact: true })).toHaveCount(0);
});
