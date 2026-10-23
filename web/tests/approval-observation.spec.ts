import { expect, test, type Page } from "@playwright/test";
import { setup } from "./collaboration-fixture";
import { registryVersion, routeRegistryVersions } from "./registry-fixture";

const entry = (id: string, hook = "approval_observation", mode = "shadow") => ({
  id,
  version: "1.0.0",
  kind: "decider",
  name: { en: id },
  description: { en: id },
  capabilities: [],
  tags: [],
  languages: [],
  schema: {},
  config: {
    hook,
    mode,
    routes: [{ model: "typesafe/jev-1.14", hosts: ["typesafe"] }],
  },
});
const rate = (numerator = 0, denominator = 0) => ({
  numerator,
  denominator,
  value: denominator ? numerator / denominator : null,
});
function metrics() {
  return {
    total_approvals: 4,
    available_observations: 2,
    explicit_labels: 1,
    comparable: 1,
    confusion: {
      shadow_allow_human_allow: 0,
      shadow_allow_human_deny: 1,
      shadow_block_human_allow: 0,
      shadow_block_human_deny: 0,
    },
    agreement: rate(0, 1),
    false_allow_among_human_denials: rate(1, 1),
    false_allow_among_shadow_allows: rate(1, 1),
    false_block_among_human_approvals: rate(),
    availability: rate(2, 4),
    label_coverage: rate(1, 4),
    comparable_coverage: rate(1, 4),
    exclusions: {
      rule: 1,
      auto: 0,
      bypass: 0,
      expired: 1,
      cancelled: 0,
      revoked: 0,
      pending: 1,
      missing_observations: 1,
      unavailable_observations: 1,
      pending_observations: 0,
    },
    risk_bands: [
      {
        lower_inclusive: 0,
        upper: 0.2,
        upper_inclusive: false,
        observations: 2,
        explicit_labels: 1,
        human_denials: 1,
        empirical_human_denial_rate: rate(1, 1),
      },
    ],
  };
}
const panel = (page: Page, title: string) =>
  page
    .locator("section.panel")
    .filter({ has: page.getByRole("heading", { name: title, exact: true }) });
async function observationAdmin(
  page: Page,
  actions: string[],
  locale: "en-US" | "ja-JP" = "en-US",
) {
  const { errors } = await setup(page, {
    subject: true,
    tenantActions: actions,
    locale,
  });
  await routeRegistryVersions(page, () => [
    registryVersion(entry("approved-observer")),
    registryVersion(entry("unapproved-observer"), { approval: null }),
    registryVersion(entry("auto-decider", "approval", "enforce")),
    registryVersion(entry("compaction", "compaction", "shadow")),
  ]);
  let setting = {
    revision: 7,
    decider: null as { id: string; version: string } | null,
    node: "default-node",
  };
  const updates: unknown[] = [];
  let reports = 0;
  let autoUpdates = 0;
  await page.route("**/api/tenants/acme/approval-decider*", async (route) => {
    if (route.request().method() === "PUT") autoUpdates++;
    return route.fulfill({
      json: {
        decider: { id: "auto-decider", version: "1.0.0" },
        built_in: false,
        registered: true,
        approved: true,
        revision: 2,
      },
    });
  });
  await page.route("**/api/tenants/acme/approval-observer*", async (route) => {
    if (route.request().method() === "PUT") {
      const body = route.request().postDataJSON();
      updates.push(body);
      setting = {
        ...setting,
        decider: body.decider,
        revision: setting.revision + 1,
      };
    }
    return route.fulfill({ json: setting });
  });
  await page.route(
    "**/api/tenants/acme/approval-observations/report?*",
    async (route) => {
      reports++;
      const query = new URL(route.request().url()).searchParams;
      return route.fulfill({
        json: {
          range: { start: query.get("start"), end: query.get("end") },
          metrics: metrics(),
          groups: [
            "tool_call",
            "outbound",
            "web_search",
            "page_reading",
            "generation",
          ].map((kind) => ({
            group: {
              decider: { id: "approved-observer", version: "1.0.0" },
              configuration_digest: "sha256:configuration",
              definition_digest: "sha256:definition",
              settings_revision: 7,
              kind,
              operation_class: null,
              effect_class: null,
              route: 0,
              provider_contract: "typesafe.jev/1",
              model: "typesafe/jev-1.14",
              hosts: ["typesafe"],
              effective_threshold: "3fe0000000000000",
            },
            metrics: metrics(),
          })),
        },
      });
    },
  );
  await page.goto("/trust?view=authorization");
  return {
    errors,
    updates,
    reports: () => reports,
    autoUpdates: () => autoUpdates,
  };
}

test("observer selection is an explicit approved Shadow version separate from Auto", async ({
  page,
}) => {
  const state = await observationAdmin(page, [
    "approval.read",
    "tenant.update",
  ]);
  const observer = panel(page, "Independent Shadow approval observer");
  const select = observer.getByLabel(
    "Approved exact Shadow observation Decider",
  );
  await expect(select.locator("option")).toHaveText([
    "Disabled (no extra calls)",
    "approved-observer · approved-observer@1.0.0",
  ]);
  await select.selectOption("approved-observer@1.0.0");
  await expect(observer).toContainText("Saved for future observations.");
  expect(state.updates).toEqual([
    {
      expected_revision: 7,
      decider: { id: "approved-observer", version: "1.0.0" },
    },
  ]);
  expect(state.autoUpdates()).toBe(0);
  await expect(observer).toContainText("typesafe/jev-1.14");
  await select.selectOption("");
  await expect.poll(() => state.updates.length).toBe(2);
  expect(state.updates[1]).toEqual({ expected_revision: 8, decider: null });
  expect(state.errors).toEqual([]);
});

test("aggregate report shows five kinds, 2x2, missingness and zero-denominator N/A", async ({
  page,
}) => {
  const state = await observationAdmin(page, ["approval.read"]);
  const report = panel(page, "Tenant Shadow approval report");
  await expect(report).toContainText("50% (2 / 4)");
  await expect(report).toContainText("N/A (0 / 0)");
  await expect(report.getByRole("table").first()).toContainText("Human deny");
  for (const kind of [
    "Tool call",
    "Outbound",
    "Web search",
    "Page reading",
    "Generation",
  ])
    await expect(
      report.locator("summary").filter({ hasText: kind }),
    ).toHaveCount(1);
  await report
    .getByText("Exclusions and missingness (counts may overlap)", {
      exact: true,
    })
    .first()
    .click();
  await expect(report).toContainText("Missing observations");
  await expect(report).toContainText("Unavailable observations");
  const before = state.reports();
  await report.getByLabel("Start date (UTC, inclusive)").fill("2026-01-01");
  await report.getByLabel("End date (UTC, exclusive)").fill("2026-03-01");
  await report.getByRole("button", { name: "Show report" }).click();
  await expect(report.getByRole("alert")).toContainText("at most 31 days");
  expect(state.reports()).toBe(before);
  await expect(
    panel(page, "Independent Shadow approval observer").getByRole("combobox"),
  ).toHaveCount(0);
  expect(state.errors).toEqual([]);
});

test("ordinary members do not request Tenant aggregate reports", async ({
  page,
}) => {
  const state = await observationAdmin(page, ["registry.create"]);
  await expect(panel(page, "Tenant Shadow approval report")).toHaveCount(0);
  expect(state.reports()).toBe(0);
});

test("Generation comparisons show bounded evidence without copying raw state or IDs", async ({
  page,
}) => {
  const { errors } = await setup(page, { subject: true });
  await page.route("**/api/generation/acme/**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/requests"))
      return route.fulfill({
        json: [
          {
            id: "private-generation-id",
            task_id: "task-0",
            policy_id: "policy",
            policy_revision: 3,
            status: "PENDING_APPROVAL",
            reason: "Observe this generation",
            expires_at: "2026-10-30T00:00:00Z",
            subject_chain: [],
            depth: 1,
            token_limit: 100,
            definition: {
              name: { en: "Generated agent" },
              capabilities: [],
              config: {},
            },
          },
        ],
      });
    return route.fulfill({
      json: path.endsWith("/policies") || path.endsWith("/history") ? [] : null,
    });
  });
  await page.route(
    "**/api/tenants/acme/approval-observations/generation/*",
    (route) =>
      route.fulfill({
        json: {
          kind: "generation",
          settings_revision: 7,
          decider: {
            id: "observer",
            version: "2.0.0",
            registry_deployment: "aidash://observer-registry",
          },
          definition_digest: "sha256:definition",
          configuration_digest: "sha256:configuration",
          human_label: "deny",
          outcome: { status: "available", verdict: "allowed" },
          routes: [
            {
              route: 0,
              provider_contract: "typesafe.jev/1",
              model: "typesafe/jev-1.14",
              hosts: ["typesafe"],
              effective_threshold: "3fe0000000000000",
              answers: Object.fromEntries(
                [
                  "data_exfiltration",
                  "destructive",
                  "beyond_request",
                  "injected",
                  "boundary",
                ].map((key) => [key, "3fc999999999999a"]),
              ),
            },
          ],
          state: "PRIVATE_STATE_SENTINEL",
          arguments: "PRIVATE_ARGUMENTS_SENTINEL",
          observation: "PRIVATE_OBSERVATION_ID",
        },
      }),
  );
  await page.goto("/generation");
  await page.getByRole("button", { name: /Observe this generation/ }).click();
  const comparison = page
    .getByRole("dialog")
    .locator("details")
    .filter({
      has: page.locator("summary", {
        hasText: "Shadow observation and human decision",
      }),
    });
  await comparison.locator("summary").click();
  await expect(comparison).toContainText("observer@2.0.0");
  await expect(comparison).toContainText("aidash://observer-registry");
  await expect(comparison).toContainText("Explicit human label");
  await expect(comparison).toContainText("Deny");
  await expect(comparison).toContainText("0.2");
  await expect(comparison).not.toContainText("PRIVATE_");
  await expect(comparison).not.toContainText("private-generation-id");
  expect(errors).toEqual([]);
});

test("unlabelled and empty reports remain explicit no-data states", async ({
  page,
}) => {
  await observationAdmin(page, ["approval.read"]);
  let empty = false;
  await page.route(
    "**/api/tenants/acme/approval-observations/report?*",
    async (route) => {
      const query = new URL(route.request().url()).searchParams;
      const unlabelled = {
        ...metrics(),
        total_approvals: empty ? 0 : 4,
        explicit_labels: 0,
        available_observations: empty ? 0 : 2,
        availability: empty ? rate() : rate(2, 4),
        comparable: 0,
        confusion: {
          shadow_allow_human_allow: 0,
          shadow_allow_human_deny: 0,
          shadow_block_human_allow: 0,
          shadow_block_human_deny: 0,
        },
        agreement: rate(),
        false_allow_among_human_denials: rate(),
        false_allow_among_shadow_allows: rate(),
        false_block_among_human_approvals: rate(),
        label_coverage: empty ? rate() : rate(0, 4),
        comparable_coverage: empty ? rate() : rate(0, 4),
        risk_bands: [],
        exclusions: {
          rule: 0,
          auto: 0,
          bypass: 0,
          expired: 0,
          cancelled: 0,
          revoked: 0,
          pending: empty ? 0 : 4,
          missing_observations: empty ? 0 : 2,
          unavailable_observations: 0,
          pending_observations: 0,
        },
      };
      await route.fulfill({
        json: {
          range: { start: query.get("start"), end: query.get("end") },
          metrics: unlabelled,
          groups: [],
        },
      });
    },
  );
  await page.reload();
  let report = panel(page, "Tenant Shadow approval report");
  await expect(report).toContainText(
    "Insufficient data: no observation paired with an explicit human decision.",
  );
  await expect(report).toContainText("N/A (0 / 0)");
  empty = true;
  await page.reload();
  report = panel(page, "Tenant Shadow approval report");
  await expect(report).toContainText("No approvals in this interval.");
});

test("observer settings and report labels remain accessible in Japanese", async ({
  page,
}) => {
  await observationAdmin(page, ["approval.read", "tenant.update"], "ja-JP");
  const observer = panel(page, "独立した Shadow 承認の観測");
  await expect(
    observer.getByLabel("承認済みの正確な Shadow 観測 Decider"),
  ).toBeVisible();
  await expect(observer).toContainText("無効（追加呼び出しなし）");
  const report = panel(page, "Tenant の Shadow 承認レポート");
  await expect(report.getByLabel("開始日（UTC・含む）")).toBeVisible();
  await expect(
    report.locator("summary").filter({ hasText: "ページ読み取り" }),
  ).toHaveCount(1);
  await expect(report.getByRole("table").first()).toContainText("人が拒否");
});
