import { expect, test, type Page, type Route } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";
import { setup } from "./collaboration-fixture";

// Evidence Revocation and Tenant Retention Rules in the dashboard (#295). The
// server's audiences and validation are covered by the Rust integration
// tests; these stubs pin what each audience is offered.

const RUN = "3f2a9c1e-7b4d-4e8a-9c2b-1d5e6f7a8b9c";
const HOUR = 3_600;
const DAY = 86_400;

const periods = (after: number, max: number) => ({
  document_snapshots: { after_run_seconds: after, max_age_seconds: max },
});

const record = (run: string, revokedBy = "subject:alpha/alice") => ({
  run_id: run,
  tenant: "alpha",
  revoked_by: revokedBy,
  reason: "The rights holder asked us to remove the page.",
  revoked_at: "2026-10-17T09:00:00Z",
  removed: {
    sources: 2,
    snapshots: 1,
    fragments: 3,
    attempts: 2,
    invocations: 2,
    snapshot_digests: ["sha256:abc"],
  },
});

function research(revokedAt: string | null, can: boolean, searches = 2) {
  return {
    limits: {
      search_attempts: 20,
      fetch_attempts: 50,
      sources: 100,
      snapshots: 50,
      snapshot_bytes: 8388608,
    },
    usage: {
      search_attempts: searches,
      fetch_attempts: 0,
      sources: searches,
      snapshots: 0,
      snapshot_bytes: 0,
    },
    estimated_micro_usd: 0,
    uncertain_attempts: 0,
    in_flight_attempts: 0,
    url_intents: [],
    revoked_at: revokedAt,
    can_revoke: can,
    can_read_revocation: can,
  };
}

async function openRun(page: Page) {
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Execution history", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByRole("button")
    .filter({ hasText: "Researcher" })
    .click();
}

test("a permitted reader revokes a Run's evidence with a reason and then sees the record", async ({
  page,
}) => {
  const { errors } = await setup(page, { locale: "en-US" });
  let revokedAt: string | null = null;
  const posted: unknown[] = [];
  await page.route("**/api/runs/*/research**", (route: Route) => {
    const request = route.request();
    const path = new URL(request.url()).pathname;
    const run = path.split("/")[3];
    if (path.endsWith("/research"))
      return route.fulfill({ json: research(revokedAt, true) });
    if (request.method() === "POST") {
      posted.push(request.postDataJSON());
      revokedAt = record(run).revoked_at;
      return route.fulfill({ json: record(run) });
    }
    return revokedAt
      ? route.fulfill({ json: record(run) })
      : route.fulfill({
          status: 404,
          json: { error: "revocation unavailable" },
        });
  });
  await openRun(page);
  const section = page.getByRole("region", {
    name: "Research evidence",
    exact: true,
  });
  await section
    .getByRole("button", { name: "Revoke research evidence…" })
    .click();
  const dialog = page.getByRole("dialog", {
    name: "Revoke this Run's research evidence",
  });
  await expect(dialog).toContainText("Revocation cannot be undone.");
  await expect(dialog).toContainText(
    "Do not paste URLs, page content or personal data",
  );
  const confirm = dialog.getByRole("button", { name: "Revoke evidence" });
  await expect(confirm).toBeDisabled();
  await dialog
    .getByLabel("Reason (required)")
    .fill("The rights holder asked us to remove the page.");
  await confirm.click();
  await expect(dialog).toHaveCount(0);
  expect(posted).toEqual([
    { reason: "The rights holder asked us to remove the page." },
  ]);
  await expect(section.getByRole("status")).toContainText(
    "This Run's research evidence was revoked",
  );
  await expect(section).toContainText(
    "The rights holder asked us to remove the page.",
  );
  await expect(
    section.getByRole("button", { name: "Revoke research evidence…" }),
  ).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("a reader without the permission is not offered revocation and sees only that it happened", async ({
  page,
}) => {
  await setup(page, { locale: "en-US" });
  let revokedAt: string | null = null;
  const reads: string[] = [];
  await page.route("**/api/runs/*/research**", (route: Route) => {
    const path = new URL(route.request().url()).pathname;
    reads.push(path);
    return route.fulfill({ json: research(revokedAt, false) });
  });
  await openRun(page);
  await expect(page.getByRole("dialog")).toContainText("Researcher");
  const section = page.getByRole("region", {
    name: "Research evidence",
    exact: true,
  });
  await expect.poll(() => reads.length).toBeGreaterThan(0);
  await expect(section).toHaveCount(0);
  revokedAt = "2026-10-17T09:00:00Z";
  await expect(section.getByRole("status")).toContainText(
    "This Run's research evidence was revoked",
    { timeout: 10_000 },
  );
  // Who revoked it and why are only for the records' audience.
  expect(reads.filter((path) => path.endsWith("/research-revocation"))).toEqual(
    [],
  );
});

test("a reader who loses the permission no longer sees the record read before", async ({
  page,
}) => {
  await setup(page, { locale: "en-US" });
  let can = true;
  await page.route("**/api/runs/*/research**", (route: Route) => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/research"))
      return route.fulfill({
        json: research("2026-10-17T09:00:00Z", can),
      });
    return route.fulfill({ json: record(path.split("/")[3]) });
  });
  await openRun(page);
  const section = page.getByRole("region", {
    name: "Research evidence",
    exact: true,
  });
  const reason = "The rights holder asked us to remove the page.";
  await expect(section).toContainText(reason);
  can = false;
  // The next refresh drops the permission; the cached record stays hidden.
  await expect(section).not.toContainText(reason, { timeout: 10_000 });
  await expect(section.getByRole("status")).toContainText(
    "This Run's research evidence was revoked",
  );
});

type View = {
  tenant_rules: ReturnType<typeof periods> | null;
  can: boolean;
};

async function trust(
  page: Page,
  access: Record<string, unknown>,
  view: View,
  options: { history?: number } = {},
) {
  const subject =
    access.kind === "subject"
      ? { tenant: String(access.tenant), name: String(access.subject) }
      : undefined;
  await installBearerDashboard(page, "fixture", subject);
  await page.addInitScript(() =>
    localStorage.setItem("aidash-locale", "en-US"),
  );
  const writes: { path: string; body: unknown }[] = [];
  const deployment = periods(DAY, 7 * DAY);
  const effective = () =>
    view.tenant_rules
      ? periods(
          Math.min(view.tenant_rules.document_snapshots.after_run_seconds, DAY),
          Math.min(
            view.tenant_rules.document_snapshots.max_age_seconds,
            7 * DAY,
          ),
        )
      : deployment;
  const retention = () => ({
    tenant: "alpha",
    deployment,
    tenant_rules: view.tenant_rules,
    effective: effective(),
    can_update: view.can,
    can_read_history: view.can,
    can_read_revocations: view.can,
  });
  const history = Array.from({ length: options.history ?? 1 }, (_, index) => ({
    rules: index === 0 ? periods(HOUR, DAY) : null,
    changed_at: new Date(Date.UTC(2026, 9, 17, 9, 0, index)).toISOString(),
    changed_by: "subject:alpha/alice",
  }));
  await page.route("**/api/**", async (route) => {
    const request = route.request();
    const url = new URL(request.url());
    const path = url.pathname;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/session")
      return route.fulfill({ json: { access, node_id: "aidash://test" } });
    if (path === "/api/state")
      return route.fulfill({
        json: {
          access,
          node: {
            id: "aidash://test",
            endpoint: "http://localhost",
            protocol_version: "0.1",
            capabilities: [],
            clusters: [],
          },
          registry: [],
          workspaces: [],
          tasks: [],
          artifacts: [],
          events: [],
          runs: [],
          conversations: [],
          human_requests: [],
          installations: [],
          peers: [],
        },
      });
    if (path === "/api/mesh")
      return route.fulfill({ json: { nodes: [], errors: [] } });
    if (path === "/api/tenants/alpha/retention") {
      if (request.method() === "PUT") {
        const body = request.postDataJSON();
        writes.push({ path, body });
        view.tenant_rules = body.rules;
      }
      return route.fulfill({ json: retention() });
    }
    if (path === "/api/tenants/alpha/retention-history") {
      const offset = Number(url.searchParams.get("offset"));
      const limit = Number(url.searchParams.get("limit"));
      return route.fulfill({ json: history.slice(offset, offset + limit) });
    }
    if (path === "/api/tenants/alpha/research-revocations")
      return route.fulfill({
        json: [record(RUN, "deployment-administrator:7c1d")],
      });
    if (path.endsWith("/research-revocation")) {
      const body = request.postDataJSON();
      writes.push({ path, body });
      return route.fulfill({ status: 404, json: { error: "run unavailable" } });
    }
    return route.fulfill({ json: [] });
  });
  return { writes };
}

const SUBJECT = { kind: "subject", tenant: "alpha", subject: "alice" };

test("a Tenant shortens its periods within the Deployment's ceiling after reviewing what is removed", async ({
  page,
}) => {
  const { writes } = await trust(page, SUBJECT, {
    tenant_rules: null,
    can: true,
  });
  await page.goto("/trust?view=evidence");
  const retention = page.getByRole("region", { name: "Snapshot retention" });
  await expect(retention).toContainText("1 day (Deployment ceiling 1 day)");
  await expect(retention).toContainText("The Deployment's periods apply.");
  await expect(retention).toContainText(
    "Saving makes both periods this Tenant's own.",
  );

  // Longer than the ceiling is refused before anything is sent.
  const after = retention.getByRole("spinbutton", {
    name: "Kept after the Run ends",
    exact: true,
  });
  await after.fill("2");
  await retention.getByRole("button", { name: "Review change" }).click();
  await expect(retention).toContainText(
    "This is longer than the Deployment's ceiling.",
  );
  expect(writes).toEqual([]);

  await after.fill("1");
  await retention
    .getByRole("combobox", { name: "Unit" })
    .first()
    .selectOption("hours");
  await retention.getByRole("button", { name: "Review change" }).click();
  await expect(retention).toContainText("1 day → 1 hour");
  await expect(retention).toContainText(
    "Content already kept beyond the new periods is removed at the next hourly sweep.",
  );
  await retention.getByRole("button", { name: "Save periods" }).click();
  await expect(retention).toContainText("This Tenant's own periods apply.");
  expect(writes).toEqual([
    {
      path: "/api/tenants/alpha/retention",
      body: { rules: periods(HOUR, 7 * DAY) },
    },
  ]);

  // Clearing lengthens nothing already kept, so it does not warn.
  await retention
    .getByRole("button", { name: "Clear to the Deployment's periods" })
    .click();
  await expect(retention).toContainText("1 hour → 1 day");
  await expect(retention).not.toContainText("is removed at the next");
  await retention.getByRole("button", { name: "Save periods" }).click();
  await expect(retention).toContainText("The Deployment's periods apply.");
  expect(writes[1].body).toEqual({ rules: null });
});

test("history and revocation records page through the Tenant's audit history", async ({
  page,
}) => {
  await trust(
    page,
    SUBJECT,
    { tenant_rules: null, can: true },
    { history: 51 },
  );
  await page.goto("/trust?view=evidence");
  const history = page.getByRole("region", { name: "Retention history" });
  await expect(history.locator("details")).toHaveCount(50);
  await history.getByRole("button", { name: "Load more" }).click();
  await expect(history.locator("details")).toHaveCount(51);
  await expect(history.getByRole("button", { name: "Load more" })).toHaveCount(
    0,
  );
  await expect(history.locator("details").first()).toContainText(
    "Set to 1 hour / 1 day · alice",
  );
  const records = page.getByRole("region", { name: "Evidence revocations" });
  // A Deployment Administrator's identity is not shown to the Tenant.
  await expect(records.locator("details")).toContainText(
    `Run ${RUN.slice(0, 8)} · Deployment Administrator`,
  );
  await expect(records).not.toContainText("7c1d");
  await expect(
    records.getByLabel("Revoke by Run ID", { exact: true }),
  ).toHaveCount(0);
});

test("a subject without the permissions reads the periods and nothing else", async ({
  page,
}) => {
  await trust(page, SUBJECT, {
    tenant_rules: periods(2 * DAY, DAY),
    can: false,
  });
  await page.goto("/trust?view=evidence");
  const retention = page.getByRole("region", { name: "Snapshot retention" });
  await expect(retention).toContainText("1 day (Deployment ceiling 1 day)");
  // A stored period above a lowered ceiling is capped by it.
  await expect(retention).toContainText(
    "This Tenant's periods exceed the Deployment's ceiling, so the ceiling applies.",
  );
  await expect(
    retention.getByRole("button", { name: "Review change" }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("region", { name: "Retention history" }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("region", { name: "Evidence revocations" }),
  ).toHaveCount(0);
});

test("a Deployment Administrator revokes by Run ID only within the chosen Tenant", async ({
  page,
}) => {
  const { writes } = await trust(
    page,
    { kind: "deployment_administrator" },
    { tenant_rules: null, can: true },
  );
  await page.goto("/trust?view=evidence");
  await page.getByLabel("Tenant", { exact: true }).last().fill("alpha");
  await page.getByRole("button", { name: "Open", exact: true }).last().click();
  const records = page.getByRole("region", { name: "Evidence revocations" });
  await expect(records.locator("details")).toContainText(
    "Deployment Administrator (7c1d)",
  );
  await records.getByLabel("Revoke by Run ID", { exact: true }).fill(RUN);
  await records.getByRole("button", { name: "Review revocation" }).click();
  const dialog = page.getByRole("dialog", {
    name: "Revoke this Run's research evidence",
  });
  await expect(dialog).toContainText("alpha");
  await expect(dialog).toContainText(RUN);
  await dialog.getByLabel("Reason (required)").fill("Agreement ended.");
  await dialog.getByRole("button", { name: "Revoke evidence" }).click();
  // The stub answers as the server does for a Run of another Tenant.
  await expect(dialog.getByRole("alert")).toContainText("run unavailable");
  expect(writes).toEqual([
    {
      path: `/api/runs/${RUN}/research-revocation`,
      body: { reason: "Agreement ended.", tenant: "alpha" },
    },
  ]);
});
