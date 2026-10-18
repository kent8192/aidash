import { expect, test } from "@playwright/test";
import { CITED, UNAVAILABLE, setup } from "./collaboration-fixture";

const COPY = {
  "en-US": {
    source: (ordinal: number) => `Source ${ordinal}`,
    excerpt: "Excerpt the agent read",
    context: "Surrounding text (not given to the agent)",
    unavailable: "This source is unavailable.",
    results: /^Results/,
    pageImage: "Page the agent viewed",
    pageImageAlt: "Page 2 as the agent viewed it",
  },
  "ja-JP": {
    source: (ordinal: number) => `出典 ${ordinal}`,
    excerpt: "エージェントが読んだ抜粋",
    context: "前後の文章（エージェントには渡していません）",
    unavailable: "この出典は利用できません。",
    results: /^成果物/,
    pageImage: "エージェントが見たページ",
    pageImageAlt: "エージェントが見た 2 ページ目",
  },
} as const;

const EVIDENCE = {
  evidence_ref: CITED,
  run_id: "run-0",
  document_id: "document-one",
  source_id: "source-one",
  url: "https://research-fixture.example/strasse",
  requested_url: "https://research-fixture.example/start",
  title: "Die Straße",
  media_type: "text/plain",
  fetched_at: "2026-09-22T10:10:00Z",
  completeness: "complete",
  text_digest: "sha256:text",
  text: "Alpha <script>inert</script>",
  line_start: 1,
  line_end: 1,
  pages: [2],
  context: {
    before: [],
    after: [
      { text: "Die Straße", page: 2 },
      { text: "Omega line", page: 2 },
    ],
  },
};

for (const locale of ["en-US", "ja-JP"] as const) {
  test(`run-published citations open their evidence in ${locale}`, async ({
    page,
  }) => {
    const copy = COPY[locale];
    const { errors } = await setup(page, { citations: true, locale });
    const reads: string[] = [];
    let revoked = false;
    await page.route("**/api/runs/*/evidence/*", (route) => {
      const path = new URL(route.request().url()).pathname;
      reads.push(path);
      return !revoked && path === `/api/runs/run-0/evidence/${CITED}`
        ? route.fulfill({ json: EVIDENCE })
        : route.fulfill({
            status: 404,
            json: { error: "evidence unavailable" },
          });
    });
    await page.goto("/collaboration?channel=workspace-one");

    // Arrange: chips number references by first use; a repeat keeps its number.
    const published = page.locator("article", {
      hasText: "Evidence is ready",
    });
    await expect(
      published.getByRole("button", { name: copy.source(1), exact: true }),
    ).toHaveCount(2);
    await expect(
      published.getByRole("button", { name: copy.source(2), exact: true }),
    ).toHaveCount(1);
    await expect(published).not.toContainText("[[cite:");
    // A person's pasted marker is not a citation: it stays literal text.
    const pasted = page.locator("article", { hasText: "Pasted" });
    await expect(pasted).toContainText(`[[cite:${CITED}]]`);
    await expect(
      pasted.getByRole("button", { name: copy.source(1) }),
    ).toHaveCount(0);
    // A citation inside bold or italics keeps the surrounding formatting.
    const formatted = page.locator("article", { hasText: "Formatted" });
    await expect(
      formatted
        .locator("strong")
        .getByRole("button", { name: copy.source(1), exact: true }),
    ).toHaveCount(1);
    await expect(
      formatted
        .locator("em")
        .getByRole("button", { name: copy.source(2), exact: true }),
    ).toHaveCount(1);
    await expect(formatted).not.toContainText("**");
    await expect(formatted).not.toContainText("[[cite:");
    expect(reads, "nothing is read before a chip opens").toEqual([]);

    // Act: open the first citation.
    await published
      .getByRole("button", { name: copy.source(1), exact: true })
      .first()
      .click();

    // Assert: the excerpt as delivered, its source and the undelivered context.
    const popover = page.getByRole("dialog", { name: copy.source(1) });
    await expect(
      popover.getByRole("link", { name: "Die Straße" }),
    ).toHaveAttribute("href", EVIDENCE.url);
    await expect(popover).toContainText(EVIDENCE.requested_url);
    await expect(
      popover.getByRole("region", { name: copy.excerpt }),
    ).toContainText("Alpha <script>inert</script>");
    await popover.getByText(copy.context).click();
    await expect(popover).toContainText("Omega line");
    expect(reads).toEqual([`/api/runs/run-0/evidence/${CITED}`]);
    if (locale === "en-US")
      await page.screenshot({ path: "test-results/citations-popover.png" });
    await page.keyboard.press("Escape");

    // Reopening reads again: access lost since the last opening withholds
    // the evidence instead of showing the earlier read.
    revoked = true;
    await published
      .getByRole("button", { name: copy.source(1), exact: true })
      .first()
      .click();
    const reopened = page.getByRole("dialog", { name: copy.source(1) });
    await expect(reopened.getByRole("status")).toHaveText(copy.unavailable);
    await expect(reopened).not.toContainText(EVIDENCE.text);
    expect(reads).toEqual([
      `/api/runs/run-0/evidence/${CITED}`,
      `/api/runs/run-0/evidence/${CITED}`,
    ]);
    await page.keyboard.press("Escape");
    revoked = false;

    // An unavailable source is a quiet state, never an error.
    await published
      .getByRole("button", { name: copy.source(2), exact: true })
      .click();
    const missing = page.getByRole("dialog", { name: copy.source(2) });
    await expect(missing.getByRole("status")).toHaveText(copy.unavailable);
    await expect(missing.getByRole("alert")).toHaveCount(0);
    expect(reads.at(-1)).toBe(`/api/runs/run-0/evidence/${UNAVAILABLE}`);
    await page.keyboard.press("Escape");

    // The Task's text result shows the same citations.
    await page.getByRole("tab", { name: copy.results }).click();
    await page.getByText("answer.txt").click();
    await expect(
      page
        .locator("li", { hasText: "answer.txt" })
        .getByRole("button", { name: copy.source(1), exact: true }),
    ).toBeVisible();
    if (locale === "en-US")
      await page.screenshot({ path: "test-results/citations-viewer.png" });
    expect(errors).toEqual([]);
  });
}

/** A one-pixel PNG, as the view operation stores it. */
const PIXEL =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

const PAGE_IMAGE = {
  ...EVIDENCE,
  url: "https://research-fixture.example/annual-report.pdf",
  requested_url: "https://research-fixture.example/annual-report.pdf",
  title: "Annual report",
  media_type: "application/pdf",
  text: "",
  line_start: 0,
  line_end: 0,
  pages: [2],
  context: null,
  image: {
    page: 2,
    media_type: "image/png",
    width: 1,
    height: 1,
    digest: "sha256:image",
    data: PIXEL,
  },
};

for (const locale of ["en-US", "ja-JP"] as const) {
  test(`a cited page view shows the image the agent saw in ${locale}`, async ({
    page,
  }) => {
    const copy = COPY[locale];
    const { errors } = await setup(page, { citations: true, locale });
    await page.route("**/api/runs/*/evidence/*", (route) =>
      route.fulfill({ json: PAGE_IMAGE }),
    );
    await page.goto("/collaboration?channel=workspace-one");

    await page
      .locator("article", { hasText: "Evidence is ready" })
      .getByRole("button", { name: copy.source(1), exact: true })
      .first()
      .click();

    const popover = page.getByRole("dialog", { name: copy.source(1) });
    const image = popover
      .getByRole("region", { name: copy.pageImage })
      .getByRole("img", { name: copy.pageImageAlt });
    await expect(image).toHaveAttribute(
      "src",
      `data:image/png;base64,${PIXEL}`,
    );
    // A page image has no passage and no surrounding text to offer.
    await expect(
      popover.getByRole("region", { name: copy.excerpt }),
    ).toHaveCount(0);
    await expect(popover.getByText(copy.context)).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}
