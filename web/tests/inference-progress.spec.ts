import { expect, test } from "@playwright/test";
import { setup } from "./collaboration-fixture";

const EVENTS: Record<string, string> = {
  started: "inference.outcome",
  outcome: "inference.outcome",
  text: "inference.delta",
  tool_call: "inference.tool_call",
};

function row(seq: number, attempt: string, kind: string, item: object) {
  const envelope = JSON.stringify({
    specversion: "1.0",
    id: `run-0:${seq}`,
    source: "aidash://home",
    type: "aidash.inference.progress.v1",
    subject: "run-0",
    time: "2026-10-10T00:00:00Z",
    datacontenttype: "application/json",
    data: { attempt_id: attempt, seq, kind, item },
  });
  return `id: ${seq}\r\nevent: ${EVENTS[kind]}\r\ndata: ${envelope}\r\n\r\n`;
}

// Attempts: an interrupted draft, a discarded attempt whose text expired, and
// the pending attempt now streaming.
const body = [
  ": keepalive\n\n",
  row(1, "old", "started", { outcome: "pending" }),
  row(2, "old", "text", { type: "text", text: "Old draft" }),
  row(3, "old", "outcome", { outcome: "interrupted", reason: "stall" }),
  row(4, "mid", "started", { outcome: "pending" }),
  `event: gap\ndata: ${JSON.stringify({ attempt_id: "mid", outcome: "discarded", from: 5, to: 6 })}\n\n`,
  row(7, "mid", "outcome", { outcome: "discarded" }),
  row(8, "new", "started", { outcome: "pending" }),
  row(9, "new", "text", { type: "text", text: "Tentative " }),
  row(10, "new", "text", { type: "text", text: "answer" }),
  row(11, "new", "tool_call", {
    type: "tool_call",
    index: 0,
    id: "call-1",
    name: "search",
    argument_bytes: 128,
  }),
  "event: error\ndata: event stream interrupted\n\n",
].join("");

const COPY = {
  "en-US": {
    history: "Execution history",
    pending: "Generating",
    tool: "assembling arguments (128 bytes)",
    interrupted: "Interrupted",
    stall: "Stalled",
    discarded: "Discarded by new input",
    gap: "Some progress expired and is no longer available.",
    started: "Generation started",
  },
  "ja-JP": {
    history: "実行履歴",
    pending: "生成中",
    tool: "引数を組み立て中（128 バイト）",
    interrupted: "中断",
    stall: "応答停止",
    discarded: "新しい入力で破棄",
    gap: "一部の進捗は保持期間を過ぎたため表示できません。",
    started: "生成を開始しました",
  },
} as const;

for (const locale of ["en-US", "ja-JP"] as const) {
  test(`run details show tentative inference progress in ${locale}`, async ({
    page,
  }) => {
    const copy = COPY[locale];
    const { errors } = await setup(page, { locale });
    const cursors: (string | null)[] = [];
    await page.route("**/api/runs/*/inference/stream", (route) => {
      cursors.push(route.request().headers()["last-event-id"] ?? null);
      return route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body,
      });
    });
    await page.goto("/collaboration?channel=workspace-one");
    await page.getByRole("button", { name: copy.history, exact: true }).click();
    await page.locator(".collab-channel .collab-task").click();
    const dialog = page.getByRole("dialog");
    const progress = dialog.locator(".inference-progress");

    const pending = progress.locator(".inference-attempt.pending");
    await expect(pending).toHaveCount(1);
    await expect(pending.locator(".inference-phase")).toHaveText(copy.pending);
    await expect(pending.locator(".inference-text")).toHaveText(
      "Tentative answer",
    );
    await expect(pending.locator(".inference-tool")).toHaveText(
      `search${copy.tool}`,
    );

    const ended = progress.locator("details.inference-attempt.ended");
    await expect(ended).toHaveCount(2);
    const interrupted = ended.first();
    await expect(interrupted.locator("summary")).toContainText(
      copy.interrupted,
    );
    await expect(interrupted.locator(".inference-reason")).toHaveText(
      copy.stall,
    );
    await interrupted.locator("summary").click();
    const struck = interrupted.locator("s.inference-text");
    await expect(struck).toHaveText("Old draft");
    await expect(struck).toHaveCSS("text-decoration-line", "line-through");
    await expect(ended.nth(1).locator("summary")).toHaveText(copy.discarded);
    await expect(progress.locator(".inference-gap")).toHaveText(copy.gap);
    await expect(dialog.locator("p.sr-only[role=status]")).toHaveText(
      copy.started,
    );

    // The finite body closes, so the reader resumes from its cursor and the
    // replayed frames do not duplicate text.
    await expect.poll(() => cursors.length).toBeGreaterThan(1);
    expect(cursors[0]).toBeNull();
    expect(cursors[1]).toBe("11");
    await expect(pending.locator(".inference-text")).toHaveText(
      "Tentative answer",
    );
    await expect(progress.locator(".inference-gap")).toHaveCount(1);
    if (locale === "en-US")
      await dialog.screenshot({
        path: "test-results/inference-progress-pending-interrupted.png",
      });
    expect(errors).toEqual([]);
  });
}

test("a pending attempt keeps streaming until its outcome after the Run ends", async ({
  page,
}) => {
  const { errors, finishRuns } = await setup(page, { locale: "en-US" });
  const cursors: (string | null)[] = [];
  let outcome = false;
  await page.route("**/api/runs/*/inference/stream", (route) => {
    const cursor = route.request().headers()["last-event-id"] ?? null;
    cursors.push(cursor);
    const rows =
      cursor === null
        ? [
            row(1, "last", "started", { outcome: "pending" }),
            row(2, "last", "text", { type: "text", text: "Final answer" }),
          ]
        : outcome
          ? [row(3, "last", "outcome", { outcome: "accepted" })]
          : [];
    return route.fulfill({
      status: 200,
      contentType: "text/event-stream",
      body: [": keepalive\n\n", ...rows].join(""),
    });
  });
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Execution history", exact: true })
    .click();
  await page.locator(".collab-channel .collab-task").click();
  const dialog = page.getByRole("dialog");
  const pending = dialog.locator(".inference-attempt.pending");
  await expect(pending.locator(".inference-text")).toHaveText("Final answer");
  const pause = dialog.getByRole("button", { name: "Pause", exact: true });
  await expect(pause).toBeVisible();

  // The Run query observes the terminal transition before the stream delivers
  // the outcome written with it.
  finishRuns();
  await expect(pause).toHaveCount(0);
  const beforeOutcome = cursors.length;
  await expect.poll(() => cursors.length).toBeGreaterThan(beforeOutcome);
  outcome = true;

  await expect(pending).toHaveCount(0);
  await expect(dialog.locator("p.sr-only[role=status]")).toHaveText(
    "Response accepted",
  );
  expect(cursors[1]).toBe("2");
  expect(errors).toEqual([]);
});
