import { expect, test } from "@playwright/test";
import { setup } from "./collaboration-fixture";

for (const viewport of [
  { width: 1440, height: 700 },
  { width: 390, height: 600 },
  { width: 900, height: 400 },
]) {
  test(`long registration dialog stays contained at ${viewport.width}x${viewport.height}`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    const { errors } = await setup(page);
    await page.goto("/settings?view=registry");
    await page
      .getByRole("button", { name: "Register entity", exact: true })
      .click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("Entity type").selectOption("model");
    const heading = dialog.getByRole("heading", { name: "Register entity" });
    const headingBefore = await heading.boundingBox();
    const submit = dialog.getByRole("button", {
      name: "Register entity",
      exact: true,
    });
    await submit.scrollIntoViewIfNeeded();
    await expect(submit).toBeInViewport();
    await expect(heading).toBeInViewport();
    expect(await heading.boundingBox()).toEqual(headingBefore);
    const bounds = await dialog.boundingBox();
    const buttonBounds = await submit.boundingBox();
    expect(bounds!.y).toBeGreaterThanOrEqual(0);
    expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(viewport.height);
    expect(buttonBounds!.y + buttonBounds!.height).toBeLessThan(
      bounds!.y + bounds!.height,
    );
    expect(
      await dialog.evaluate(
        (element) => element.scrollWidth <= element.clientWidth,
      ),
    ).toBe(true);
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}

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
  await expect(page.getByRole("alert")).toContainText(
    "Your text has been kept",
  );
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
  expect(messages[0].body.idempotency_key).toBe(
    messages[1].body.idempotency_key,
  );
  expect(errors).toEqual([]);
});

test("message attachments show their names and download in the selected tab context", async ({
  page,
}) => {
  const { errors } = await setup(page, { messageAttachment: true });
  await page.goto("/collaboration?channel=workspace-one");
  const attachment = page.getByRole("button", {
    name: "Download attachment: evidence.txt",
  });
  await expect(attachment).toBeVisible();
  const [download, request] = await Promise.all([
    page.waitForEvent("download"),
    page.waitForRequest(
      (request) =>
        request.url().endsWith("/attachments/attachment-one") &&
        request.method() === "GET",
    ),
    attachment.click(),
  ]);
  expect(download.suggestedFilename()).toBe("evidence.txt");
  expect(request.headers()["x-aidash-context"]).toBe("operator");
  expect(request.headers()["authorization"]).toBeUndefined();
  expect(errors).toEqual([]);
});

test("preparing a channel does not invoke agent execution", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page);
  await page.goto("/");
  await page
    .getByRole("button", { name: "Prepare a channel", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog
    .getByRole("textbox", { name: "Title", exact: true })
    .fill("Draft research");
  await dialog
    .getByRole("textbox", { name: "Goal", exact: true })
    .fill("Discuss sources before starting");
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
  await page
    .getByRole("button", { name: "View in graph", exact: true })
    .click();
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

test("graph focus survives selection, links, reload, and history", async ({
  page,
}) => {
  const { errors } = await setup(page, { extraGraphAgent: true });
  await page.goto("/collaboration?channel=workspace-one");
  await page.getByRole("button", { name: "Tasks and results" }).click();
  await page
    .locator(".collab-channel .collab-agent")
    .filter({ hasText: "Researcher" })
    .click();
  const selector = page.getByLabel("Focus agent");
  const original = JSON.stringify([
    "entity",
    "aidash://home",
    "agent",
    "researcher",
    "1.0.0",
  ]);
  const focused = JSON.stringify([
    "entity",
    "aidash://home",
    "agent",
    'review/"[special]:/agent',
    "1.0.0",
  ]);
  await expect(selector).toHaveValue(original);
  expect(new URL(page.url()).searchParams.get("focus")).toBe(original);

  await selector.selectOption(focused);
  await expect(selector).toHaveValue(focused);
  await expect(page.locator(".collab-cytoscape canvas").first()).toBeVisible();
  expect(new URL(page.url()).searchParams.get("focus")).toBe(focused);
  const graphLink = await page
    .getByRole("link", { name: "Graph View", exact: true })
    .getAttribute("href");
  expect(new URL(graphLink!, page.url()).searchParams.get("focus")).toBe(
    focused,
  );

  await page.reload();
  await expect(selector).toHaveValue(focused);
  await page.goBack();
  await expect(selector).toHaveValue(original);
  await page.goForward();
  await expect(selector).toHaveValue(focused);

  const unavailable = JSON.stringify([
    "entity",
    "aidash://home",
    "agent",
    "unavailable",
    "1.0.0",
  ]);
  await page.goto(`/graph?focus=${encodeURIComponent(unavailable)}`);
  await expect(selector).toHaveValue("");
  await expect(
    page.getByRole("status").filter({
      hasText: "This resource is unavailable under your current access.",
    }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("channel intervention answers a human request without settings navigation", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page);
  await page.goto("/");
  await page
    .getByRole("button")
    .filter({ hasText: "Which market should I examine?" })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog
    .getByRole("textbox", { name: "Answer", exact: true })
    .fill("Japan");
  await dialog.getByRole("button", { name: "Send", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(
    submissions.some(
      (item) => item.path === "/api/human-requests/question-one/answer",
    ),
  ).toBe(true);
  expect(errors).toEqual([]);
});

test("revoked history is hidden rather than retaining a stale conversation", async ({
  page,
}) => {
  const { revokeHistory } = await setup(page);
  await page.goto("/collaboration?channel=workspace-one");
  await expect(
    page.getByText("Evidence is ready to review.", { exact: true }),
  ).toBeVisible();
  revokeHistory();
  await expect(
    page.getByText("Evidence is ready to review.", { exact: true }),
  ).toHaveCount(0);
  await expect(page.getByRole("alert")).toContainText("unavailable");
});

test("an inaccessible explicit channel is not silently replaced with another channel", async ({
  page,
}) => {
  await setup(page);
  await page.goto("/collaboration?channel=unavailable");
  await expect(
    page.getByRole("heading", { name: "# Research", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", {
      name: "This resource is unavailable under your current access.",
      exact: true,
    }),
  ).toBeVisible();
});

test("legacy management links are routed into secondary settings", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.goto("/agents");
  await expect(page).toHaveURL(/\/settings\?.*view=agents/);
  await expect(
    page.getByRole("heading", { name: "Researcher", exact: true }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("mobile conversation and channel switching do not require horizontal scrolling", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/collaboration?channel=workspace-one");
  await expect(
    page.getByRole("textbox", { name: "Message", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Channels", exact: true }).click();
  await page.getByRole("link", { name: "# Planning", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "# Planning", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  expect(errors).toEqual([]);
});

test("thread replies persist separately and channel drafts survive thread navigation", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page);
  await page.goto("/collaboration?channel=workspace-one");
  const composer = page.getByRole("textbox", { name: "Message", exact: true });
  await composer.fill("Unsent channel note");
  await page
    .getByRole("button", { name: "Reply in thread", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Thread", exact: true }),
  ).toBeVisible();
  await expect(composer).toHaveValue("Unsent channel note");
  const reply = page.getByRole("textbox", { name: "Reply", exact: true });
  await expect(reply).toHaveValue("");
  await reply.fill("Here is the supporting source.");
  await page.getByRole("button", { name: "Send reply", exact: true }).click();
  await expect(
    page.getByText("Here is the supporting source.", { exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Back to channel", exact: true })
    .click();
  await expect(composer).toHaveValue("Unsent channel note");
  await expect(
    page.getByText("Here is the supporting source.", { exact: true }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "Open thread", exact: true }).click();
  await expect(
    page.getByText("Here is the supporting source.", { exact: true }),
  ).toBeVisible();
  const posts = submissions.filter((item) =>
    item.path.endsWith("/thread-messages"),
  );
  expect(posts).toHaveLength(1);
  expect(posts[0].body.thread_id).toBe("thread-message-one");
  expect(errors).toEqual([]);
});

test("older message pages are prepended without losing the latest messages", async ({
  page,
}) => {
  const { errors } = await setup(page, { olderMessages: 45 });
  await page.goto("/collaboration?channel=workspace-one");
  await expect(page.locator(".collab-message")).toHaveCount(40);
  await expect(
    page.getByText("Previous message 0", { exact: true }),
  ).toHaveCount(0);
  await page
    .getByRole("button", { name: "Load older messages", exact: true })
    .click();
  await expect(page.locator(".collab-message")).toHaveCount(46);
  await expect(
    page.getByText("Previous message 0", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("Evidence is ready to review.", { exact: true }),
  ).toHaveCount(1);
  await expect(
    page.getByRole("button", { name: "Load older messages", exact: true }),
  ).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("remote channel participants are displayed without linking to the local graph", async ({
  page,
}) => {
  const { errors } = await setup(page, { remoteParticipant: true });
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Tasks and results", exact: true })
    .click();
  const local = page.locator("button.collab-agent");
  const remote = page.locator("div.collab-agent");
  await expect(local).toHaveCount(1);
  await expect(remote).toHaveCount(1);
  await expect(local).toContainText("Researcher · 1.0.0");
  await expect(remote).toContainText("Unavailable item · 1.0.0");
  await expect(local).toHaveJSProperty("tagName", "BUTTON");
  await expect(remote).toHaveJSProperty("tagName", "DIV");
  expect(errors).toEqual([]);
});

test("partial mesh responses tell operators which peer was omitted", async ({
  page,
}) => {
  const { errors } = await setup(page, { meshErrors: true });
  await page.goto("/graph");
  await expect(
    page.getByRole("status").filter({ hasText: "Unavailable item" }),
  ).toContainText("connection refused");
  expect(errors).toEqual([]);
});

test("remote human requests remain visible without a run in the capped snapshot", async ({
  page,
}) => {
  const { errors } = await setup(page, { unpairedRemoteRequest: true });
  await page.goto("/collaboration?channel=workspace-one");
  await expect(
    page
      .locator(".collab-attention .request-card")
      .filter({ hasText: "The run is outside the capped snapshot." }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("local run details retain execution memory", async ({ page }) => {
  const { errors } = await setup(page, { runMemory: true });
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Execution history", exact: true })
    .click();
  await page.locator(".collab-channel .collab-task").click();
  await page.getByText("Memory", { exact: true }).click();
  await expect(
    page.locator("dialog details").filter({ hasText: "Memory" }),
  ).toContainText("Retained execution memory.");
  expect(errors).toEqual([]);
});

test("the mobile channel toggle is absent outside Collaboration", async ({
  page,
}) => {
  await setup(page);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/graph");
  await expect(
    page.getByRole("button", { name: "Channels", exact: true }),
  ).toHaveCount(0);
  await page.goto("/settings?view=agents");
  await expect(
    page.getByRole("button", { name: "Channels", exact: true }),
  ).toHaveCount(0);
});

test("a same-length refreshed page follows a new latest message", async ({
  page,
}) => {
  const { errors } = await setup(page, {
    latestMessageChangesOnPoll: true,
    olderMessages: 39,
  });
  await page.goto("/collaboration?channel=workspace-one");
  await expect(page.locator(".collab-message")).toHaveCount(40);
  await expect(
    page.getByText("New latest message.", { exact: true }),
  ).toBeVisible({
    timeout: 5000,
  });
  await expect(page.locator(".collab-message")).toHaveCount(40);
  await expect
    .poll(async () =>
      page
        .locator(".collab-messages")
        .evaluate(
          (element) =>
            element.scrollHeight - element.scrollTop - element.clientHeight <
            80,
        ),
    )
    .toBe(true);
  expect(errors).toEqual([]);
});

test("revoking a thread read hides cached replies without exposing another conversation", async ({
  page,
}) => {
  const { revokeThreads } = await setup(page);
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Reply in thread", exact: true })
    .click();
  const threadPanel = page.locator(".workspace-thread-panel");
  await expect(
    threadPanel.getByText("Evidence is ready to review.", { exact: true }),
  ).toBeVisible();
  revokeThreads();
  await expect(page.getByRole("alert")).toContainText("unavailable");
  await expect(threadPanel.locator(".collab-message")).toHaveCount(0);
  await expect(
    threadPanel.getByRole("textbox", { name: "Reply", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("textbox", { name: "Message", exact: true }),
  ).toBeVisible();
});

test("attachment retries preserve uploads, files and message identity", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, { failFirstMessage: true });
  await page.goto("/");
  await page
    .getByRole("textbox", { name: "Message", exact: true })
    .fill("Please review the source.");
  await page.locator('input[type="file"]').setInputFiles({
    name: "source.txt",
    mimeType: "text/plain",
    buffer: Buffer.from("evidence"),
  });
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "Your text has been kept",
  );
  await expect(
    page.getByRole("button", { name: "Remove attachment: source.txt" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Download attachment: source.txt" }),
  ).toBeVisible();
  await expect(page.locator(".workspace-draft-files")).toHaveCount(0);
  const uploads = submissions.filter((item) =>
    item.path.endsWith("/attachments"),
  );
  const messages = submissions.filter((item) =>
    item.path.endsWith("/thread-messages"),
  );
  expect(uploads).toHaveLength(1);
  expect(uploads[0].body.size).toBe(8);
  expect(messages).toHaveLength(2);
  expect(messages[0].body).toEqual(messages[1].body);
  expect(messages[0].body.attachment_ids).toHaveLength(1);
  expect(errors).toEqual([]);
});

test("upload failures retain their key and file limits apply before network use", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, { failFirstUpload: true });
  await page.goto("/");
  const files = page.locator('input[type="file"]');
  await files.setInputFiles({
    name: "large.txt",
    mimeType: "text/plain",
    buffer: Buffer.alloc(1024 * 1024 + 1),
  });
  await expect(page.getByRole("alert")).toContainText("1 MiB");
  expect(submissions).toHaveLength(0);
  await files.setInputFiles({
    name: "source.txt",
    mimeType: "text/plain",
    buffer: Buffer.from("evidence"),
  });
  await page
    .getByRole("textbox", { name: "Message", exact: true })
    .fill("Source");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("upload unavailable");
  expect(
    submissions.filter((item) => item.path.endsWith("/thread-messages")),
  ).toHaveLength(0);
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Download attachment: source.txt" }),
  ).toBeVisible();
  const uploads = submissions.filter((item) =>
    item.path.endsWith("/attachments"),
  );
  expect(uploads).toHaveLength(2);
  expect(uploads[0].body.idempotency_key).toBe(uploads[1].body.idempotency_key);
  expect(errors).toEqual([]);
});

test("channel controls scope run changes and persist approval responses", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, { approval: true });
  await page.goto("/");
  await page
    .getByRole("button", { name: "Pause runs in this channel", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Resume runs in this channel" }),
  ).toBeEnabled();
  expect(
    submissions
      .filter((item) => item.path.endsWith("/control"))
      .map((item) => item.path),
  ).toEqual(["/api/runs/run-0/control"]);
  await page
    .getByRole("button", { name: "Resume runs in this channel" })
    .click();
  await expect(
    page.getByRole("button", { name: "Pause runs in this channel" }),
  ).toBeEnabled();
  await page.getByRole("button", { name: "Approve", exact: true }).click();
  await expect(page.locator(".workspace-approval")).toHaveCount(0);
  expect(
    submissions.find((item) => item.path.endsWith("/answer"))?.body,
  ).toEqual({ approved: true });
  expect(errors).toEqual([]);
});

test("search, notifications, thread navigation and persisted theme work", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.goto("/");
  await page
    .getByRole("searchbox", { name: "Search channels" })
    .fill("next release");
  await expect(
    page
      .locator(".collab-channel-sidebar")
      .getByRole("link", { name: "# Research" }),
  ).toHaveCount(0);
  await expect(page.getByRole("link", { name: "# Planning" })).toBeVisible();
  await page.getByRole("searchbox", { name: "Search channels" }).fill("");
  await page
    .getByLabel("Requests awaiting your input", { exact: true })
    .click();
  await page
    .locator(".workspace-popover-body")
    .getByRole("button", { name: "Which market should I examine?" })
    .click();
  await expect(page.getByRole("dialog")).toBeVisible();
  await page.keyboard.press("Escape");
  await page.getByLabel("Account settings", { exact: true }).click();
  await page.getByRole("button", { name: "Dark theme", exact: true }).click();
  await expect(page.locator(".collab-app")).toHaveAttribute(
    "data-theme",
    "dark",
  );
  await page.reload();
  await expect(page.locator(".collab-app")).toHaveAttribute(
    "data-theme",
    "dark",
  );
  await page
    .getByRole("button", { name: "Threads in this channel", exact: true })
    .click();
  await expect(
    page.getByText("No threads in the loaded history."),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Tasks and results", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Add task", exact: true }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("composer preserves IME input and safely renders inline formatting", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page);
  await page.goto("/");
  const composer = page.getByRole("textbox", { name: "Message", exact: true });
  await composer.fill("draft");
  await composer.press("Shift+Enter");
  await expect(composer).toHaveValue("draft\n");
  await composer.dispatchEvent("keydown", {
    key: "Enter",
    code: "Enter",
    isComposing: true,
  });
  expect(submissions).toHaveLength(0);
  await composer.fill("**Important** <script>alert('xss')</script>");
  await composer.press("Enter");
  await expect(composer).toHaveValue("");
  await expect(page.locator(".workspace-message-body p strong")).toHaveText(
    "Important",
  );
  await expect(page.locator(".workspace-message-body p").last()).toContainText(
    "<script>",
  );
  expect(errors).toEqual([]);
});

for (const viewport of [
  { width: 390, height: 844 },
  { width: 900, height: 400 },
]) {
  test(`composer and status remain in view at ${viewport.width}x${viewport.height}`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    const { errors } = await setup(page, { olderMessages: 45 });
    await page.goto("/");
    await expect(
      page.getByRole("textbox", { name: "Message", exact: true }),
    ).toBeInViewport();
    await expect(
      page.getByRole("button", { name: "Send", exact: true }),
    ).toBeInViewport();
    await page.getByRole("button", { name: "Show channel status" }).click();
    await expect(
      page.getByRole("button", { name: "Pause runs in this channel" }),
    ).toBeInViewport();
    await page.getByRole("button", { name: "Close", exact: true }).click();
    await expect(
      page.getByRole("textbox", { name: "Message", exact: true }),
    ).toBeInViewport();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
    expect(errors).toEqual([]);
  });
}

test("workspace reference renders light, dark and thread layouts", async ({
  page,
}, testInfo) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  const { errors } = await setup(page, {
    referenceLayout: true,
    locale: "ja-JP",
  });
  await page.goto("/");
  await expect(page.locator(".workspace-approval")).toBeVisible();
  await expect(
    page.locator(".workspace-mini-graph canvas").first(),
  ).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("workspace-light.png") });
  await page.locator(".workspace-status").screenshot({
    path: testInfo.outputPath("workspace-status.png"),
  });
  await page.locator(".collab-composer").screenshot({
    path: testInfo.outputPath("workspace-composer.png"),
  });
  await page.getByLabel("アカウント設定", { exact: true }).click();
  await page.getByRole("button", { name: "ダークテーマ", exact: true }).click();
  await page.getByLabel("アカウント設定", { exact: true }).click();
  await page.screenshot({ path: testInfo.outputPath("workspace-dark.png") });
  await page
    .locator("#message-message-one")
    .getByRole("button", { name: "スレッドで返信" })
    .click();
  await expect(
    page.getByRole("textbox", { name: "返信", exact: true }),
  ).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("workspace-thread.png") });
  await page.setViewportSize({ width: 1280, height: 960 });
  await page.screenshot({
    path: testInfo.outputPath("workspace-thread-tall.png"),
  });
  expect(errors).toEqual([]);
});

test("shared files can be inspected without leaving the workspace", async ({
  page,
}) => {
  const { errors } = await setup(page, { messageAttachment: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Artifacts", exact: true }).click();
  const files = page.locator(".workspace-file-gallery");
  await expect(
    files.getByRole("button", { name: "Download attachment: evidence.txt" }),
  ).toBeVisible();
  await files.getByRole("button", { name: "Preview: evidence.txt" }).click();
  const dialog = page.getByRole("dialog");
  await expect(
    dialog.getByRole("heading", { name: "evidence.txt" }),
  ).toBeVisible();
  await expect(dialog.locator("pre")).toHaveText("Source evidence");
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  expect(errors).toEqual([]);
});
