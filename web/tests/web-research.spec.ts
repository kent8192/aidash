import { test, expect, type Page } from "@playwright/test";
import { setup } from "./collaboration-fixture";

// Browser API contract fixtures; Rust tests own live authorization/recovery,
// and test-web-extractor.py owns the real isolated format boundary.
async function research(page: Page, citation = false) {
  const reference = "ev_00000000-0000-4000-8000-000000000001";
  const fixture = await setup(page, {
    subject: true,
    webCapabilities: true,
    webCitation: citation ? reference : undefined,
  });
  const writes: { path: string; body: Record<string, unknown> }[] = [];
  let revoked = false;
  const run = {
    id: "run-0",
    agent_id: "researcher",
    agent_version: "1.0.0",
    phase: "WAITING",
    control: "ACTIVE",
    revision: 1,
  };
  const usage = {
    search_attempts: 10,
    page_attempts: 3,
    search_limit: 10,
    page_limit: 20,
    estimated_micro_usd: 3000,
    cache_bytes: 2048,
    observation_count: 2,
    classification: "unclassified",
    context_digest: "current-context",
    enabled: true,
    search_available: false,
    page_extractor_configured: true,
    node_month_estimate_micro_usd: 20000000,
    monthly_limit_micro_usd: 20000000,
    uncertain_operations: ["unknown-operation"],
    disclosures: [
      {
        approval_id: "approval-one",
        revision: 7,
        request_digest: "exact-request",
        target: { destination: "https://example.org/", method: "GET" },
        expires_at: "2099-01-01T00:00:00Z",
      },
    ],
  };
  await page.route("**/api/**", async (route) => {
    const request = route.request(),
      path = new URL(request.url()).pathname;
    if (path.endsWith("/web/usage")) return route.fulfill({ json: usage });
    if (path.includes("/web/evidence/"))
      return revoked
        ? route.fulfill({ status: 404, json: { error: "unavailable" } })
        : route.fulfill({
            json: {
              evidence_ref: reference,
              title: "Public source",
              url: "https://example.org/source",
              fetched_at: "2026-09-30T00:00:00Z",
              text: "<script>window.fixtureExecuted=true</script>\nActually delivered evidence",
              line_start: 12,
              line_end: 13,
              pdf_pages: [2],
              completeness: "text_limit",
            },
          });
    if (path.endsWith("/web/runs") && request.method() === "GET")
      return route.fulfill({ json: [run] });
    if (
      request.method() === "POST" &&
      (path.includes("/web/") || path.endsWith("/control"))
    ) {
      const body = request.postDataJSON();
      writes.push({ path, body });
      if (path.endsWith("/decision")) usage.disclosures = [];
      if (path.endsWith("/control")) {
        run.phase = "CANCELLED";
        run.control = "CANCELLED";
      }
      return route.fulfill({
        json: path.endsWith("/web/runs") ? run : { accepted: true },
      });
    }
    return route.fallback();
  });
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Reply in thread", exact: true })
    .click();
  return {
    ...fixture,
    writes,
    revoke: () => {
      revoked = true;
    },
  };
}

test("Web-only research stays in the thread and sends exact typed approvals and stop actions", async ({
  page,
}) => {
  const { writes, errors } = await research(page);
  const panel = page.getByRole("region", { name: "Web research", exact: true });
  await expect(panel).toBeVisible();
  await expect(panel.getByText("Search 10/10", { exact: false })).toBeVisible();
  await expect(
    panel.getByText("Brave search needs current verified terms", {
      exact: false,
    }),
  ).toBeVisible();
  await expect(
    panel.getByText("Some dispatch outcomes are uncertain", { exact: false }),
  ).toBeVisible();
  await panel
    .getByRole("button", { name: "Allow this request once", exact: true })
    .click();
  expect(writes[0].body).toEqual({
    expected_revision: 7,
    request_digest: "exact-request",
    allow_once: true,
  });
  await panel
    .getByRole("button", { name: "Stop research", exact: true })
    .click();
  expect(writes[1]).toEqual({
    path: "/api/runs/run-0/control",
    body: { action: "cancel" },
  });
  await panel.getByLabel("Research Agent").selectOption("researcher@1.0.0");
  await panel.getByLabel("Research request").fill("Read this public source.");
  await panel
    .getByLabel("Public URL to open (optional)")
    .fill("https://example.org/source");
  await panel
    .getByRole("button", { name: "Start research", exact: true })
    .click();
  expect(writes[2].body).toMatchObject({
    agent: { id: "researcher", version: "1.0.0" },
    description: "Read this public source.",
    open_url: "https://example.org/source",
  });
  expect(writes[2].body.idempotency_key).toMatch(/^[0-9a-f-]{36}$/);
  expect(errors).toEqual([]);
});

test("Citations render delivered escaped excerpts and clear them when current authority is revoked", async ({
  page,
}) => {
  const { revoke, errors } = await research(page, true);
  await page
    .getByRole("button", { name: "Citation 1", exact: true })
    .first()
    .click();
  const excerpt = page.getByRole("note").first();
  await expect(
    excerpt.getByRole("link", { name: "Public source" }),
  ).toHaveAttribute("href", "https://example.org/source");
  await expect(excerpt).toContainText("Lines 12–13 · PDF 2");
  await expect(excerpt).toContainText(
    "<script>window.fixtureExecuted=true</script>",
  );
  expect(
    await page.evaluate(
      () =>
        (window as unknown as { fixtureExecuted?: boolean }).fixtureExecuted,
    ),
  ).toBeUndefined();
  revoke();
  await expect(excerpt.getByRole("alert")).toHaveText(
    "This citation is currently unavailable.",
  );
  await expect(excerpt).not.toContainText("Actually delivered evidence");
  expect(errors).toEqual([]);
});

test("Retained research remains visible after Web Agents are removed from the current catalog", async ({
  page,
}) => {
  const { revokeAgents, errors } = await research(page);
  revokeAgents();
  await page.reload();
  await page.getByRole("button", { name: "Open thread", exact: true }).click();
  const panel = page.getByRole("region", { name: "Web research", exact: true });
  await expect(panel).toBeVisible();
  await expect(panel.getByText("Search 10/10", { exact: false })).toBeVisible();
  await expect(
    panel.getByRole("button", { name: "Start research", exact: true }),
  ).toBeDisabled();
  expect(errors).toEqual([]);
});
