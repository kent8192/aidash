import { test, expect, type Page } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";

const workspace = "019c0000-0000-7000-8000-000000000001";
const participant = "019c0000-0000-7000-8000-000000000002";
const task = "019c0000-0000-7000-8000-000000000003";
const source = "019c0000-0000-7000-8000-000000000004";
const ref = (id: string) => ({ id, version: "1.0.0" });
type FixtureBody = {
  action?: {
    action: string;
    expected_revision?: number;
    before?: number | null;
    after?: string | null;
    query?: { max_tokens: number };
  };
  changes?: {
    content: {
      entities: unknown[];
      evidence: unknown[];
      verification: string;
    };
  }[];
};
async function fixture(
  page: Page,
  locale: "en-US" | "ja-JP",
  paged = false,
  visual = false,
  contextTokens = 4096,
) {
  await installBearerDashboard(page, "fixture-token");
  await page.addInitScript(
    (locale) => localStorage.setItem("aidash-locale", locale),
    locale,
  );
  const entry = (kind: string, id: string, config = {}) => ({
    ...ref(id),
    kind,
    name: { en: id, ja: id },
    description: { en: "fixture", ja: "fixture" },
    config,
    schema: {},
    capabilities: [],
    languages: ["en", "ja"],
    tags: [],
    skills: [],
  });
  const bank = {
    home: "aidash://test",
    tenant: "acme",
    workspace,
    participant,
  };
  const state = {
    access: { kind: "operator" },
    node: {
      id: bank.home,
      endpoint: "http://localhost",
      protocol_version: "0.1",
      capabilities: [],
      clusters: [],
    },
    workspaces: [
      {
        id: workspace,
        title: "Memory fixture",
        goal: "Current evidence",
        state: {},
        revision: 1,
        created_at: "2026-10-06T00:00:00Z",
      },
    ],
    tasks: [
      {
        id: task,
        workspace_id: workspace,
        title: "Assigned task",
        description: "Use current memory",
        status: "OPEN",
        owner: null,
        created_by: "operator",
        requirements: {},
        dependencies: [],
        parent_id: null,
        revision: 7,
        created_at: "2026-10-06T00:00:00Z",
      },
    ],
    registry: [
      entry("agent", "agent", {
        schema_version: 1,
        bindings: [
          {
            kind: "memory",
            target: { registry_node: "aidash://test", ...ref("memory") },
            narrow: {},
          },
        ],
        model: ref("model"),
      }),
      entry("memory", "memory", {
        engine: "hindsight_rust",
        policy: { bounds: { max_context_tokens: contextTokens } },
      }),
      entry("model", "model"),
      entry("embedding", "embedding"),
      entry("reranker", "reranker"),
      entry("tokenizer", "tokenizer"),
    ],
    events: [],
    runs: [],
    artifacts: [],
    conversations: [],
    human_requests: [],
    installations: [],
    peers: [],
  };
  const writes: { url: string; body: FixtureBody }[] = [];
  const inspections: { after?: string | null }[] = [];
  let failMutation = false;
  let withdrawn = false;
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    const body =
      route.request().method() === "POST"
        ? route.request().postDataJSON()
        : null;
    if (path === "/api/events/stream") return route.abort();
    if (path === "/api/state") return route.fulfill({ json: state });
    if (path === "/api/mesh")
      return route.fulfill({ json: { nodes: [], errors: [] } });
    if (path === "/api/discover")
      return route.fulfill({ json: { agents: [], errors: [] } });
    if (path === "/api/session")
      return route.fulfill({
        json: { access: state.access, node_id: bank.home },
      });
    if (withdrawn && path.includes("/memory/"))
      return route.fulfill({
        status: 403,
        json: { error: "Disclosure withdrawn" },
      });
    if (path.endsWith("/memory/participants"))
      return route.fulfill({
        json: {
          items: [{ id: participant, bank, agent: ref("agent"), revision: 3 }],
          next: null,
        },
      });
    if (path.endsWith("/memory-participant")) {
      if (!body)
        return route.fulfill({
          json: {
            task_revision: 7,
            participant: {
              participant_id: participant,
              participant_revision: 2,
            },
          },
        });
      writes.push({ url: path, body });
      return route.fulfill({ json: body.target });
    }
    if (path.endsWith("/memory/units/query"))
      return route.fulfill({
        json: paged
          ? [
              ...(visual
                ? [
                    {
                      id: "019c0000-0000-7000-8000-000000000005",
                      bank,
                      revision: 1,
                      content: {
                        text: "The team prefers PostgreSQL for durable memory.",
                        kind: "experience",
                        learning: "preference",
                        verification: "unverified",
                        occurred: null,
                        entities: [],
                        evidence: [],
                        links: [],
                      },
                      learned_at: "2026-10-06T00:00:00Z",
                      updated_at: "2026-10-06T00:00:00Z",
                      deleted: false,
                      stale: false,
                    },
                  ]
                : []),
              {
                id: source,
                bank,
                revision: 40,
                content: {
                  text: "A history-bearing memory unit",
                  kind: "world",
                  learning: "fact",
                  verification: "unverified",
                  occurred: visual
                    ? {
                        start: "2026-10-05T08:00:00Z",
                        end: "2026-10-05T09:00:00Z",
                      }
                    : null,
                  entities: visual
                    ? [
                        {
                          name: "PostgreSQL",
                          category: "Database",
                          aliases: ["Postgres", "ポストグレス"],
                        },
                      ]
                    : [],
                  evidence: visual
                    ? [
                        {
                          kind: "unit",
                          bank,
                          id: "019c0000-0000-7000-8000-000000000005",
                          revision: 1,
                        },
                      ]
                    : [],
                  links: visual
                    ? [
                        {
                          kind: "causes",
                          target: "019c0000-0000-7000-8000-000000000005",
                          revision: 1,
                          weight: 0.8,
                        },
                      ]
                    : [],
                },
                learned_at: "2026-10-06T00:00:00Z",
                updated_at: "2026-10-06T00:00:00Z",
                deleted: false,
                stale: false,
              },
            ]
          : [],
      });
    if (path.endsWith("/memory/units/mutate")) {
      writes.push({ url: path, body });
      if (failMutation) {
        failMutation = false;
        return route.fulfill({
          status: 503,
          json: { error: "temporary failure" },
        });
      }
      return route.fulfill({ json: [] });
    }
    if (path.endsWith("/memory/operate")) {
      const action = body.action.action;
      if (action === "recall" || action === "reflect")
        writes.push({ url: path, body });
      if (action === "history") {
        writes.push({ url: path, body });
        return route.fulfill({
          json: {
            result: "history",
            value: body.action.before
              ? [
                  {
                    unit_id: source,
                    revision: 8,
                    content: { text: "Earlier retained revision" },
                  },
                ]
              : Array.from({ length: 32 }, (_, index) => ({
                  unit_id: source,
                  revision: 40 - index,
                  content: { text: `Recent retained revision ${index}` },
                })),
          },
        });
      }

      if (action === "recall")
        return route.fulfill({
          json: {
            result: "recall",
            value: {
              status: "ready",
              units: [
                {
                  id: source,
                  revision: 40,
                  content: {
                    text: "Recalled memory with readable evidence",
                    evidence: [],
                  },
                },
              ],
            },
          },
        });
      if (action === "reflect")
        return route.fulfill({
          json: {
            result: "reflection",
            value: {
              text: "A readable reflection with cited evidence",
              evidence: [
                {
                  kind: "run",
                  id: task,
                  revision: 7,
                  digest: "sha256:observed-run",
                },
              ],
            },
          },
        });
      if (action === "usage")
        return route.fulfill({
          json: {
            result: "usage",
            value: {
              items: [
                {
                  id: task,
                  provider: ref("model"),
                  calls: 2,
                  tokens: 345,
                  cost_micros: 1200,
                  created_at: "2026-10-06T00:00:00Z",
                },
              ],
              next: null,
            },
          },
        });
      if (action === "settings")
        return route.fulfill({
          json: {
            result: "settings",
            value: { provider: ref("memory"), revision: 1 },
          },
        });
      if (action === "candidates")
        return route.fulfill({ json: { result: "candidates", value: [] } });
      if (action === "jobs")
        return route.fulfill({
          json: { result: "jobs", value: { items: [], next: null } },
        });
      if (action === "inspect") {
        inspections.push(body.action);
        return route.fulfill({
          json: {
            result: "inspection",
            value: {
              items: paged
                ? [
                    {
                      id: source,
                      revision: 40,
                      dependent_runs: body.action.after ? 23 : 0,
                    },
                  ]
                : [],
              next: paged && !body.action.after ? source : null,
              purge_jobs: [],
              model_operations: 0,
              pending_candidates: 0,
            },
          },
        });
      }
    }
    return route.fulfill({ json: [] });
  });
  await page.goto("/semantic");
  await page
    .getByLabel(locale === "ja-JP" ? "記憶の所有者" : "Memory bank")
    .selectOption(participant);
  return {
    writes,
    inspections,
    failNext: () => {
      failMutation = true;
    },
    source,
    withdraw: () => {
      withdrawn = true;
    },
  };
}

for (const locale of ["en-US", "ja-JP"] as const) {
  test(`recall and reflection respect the exact private and shared context cap (${locale})`, async ({
    page,
  }) => {
    const f = await fixture(page, locale, false, false, 512);
    const ja = locale === "ja-JP";
    for (const privateBank of [true, false]) {
      if (!privateBank) {
        await page
          .getByLabel(ja ? "記憶の所有者" : "Memory bank")
          .selectOption("");
        await page
          .getByLabel(ja ? "テナント" : "Tenant", { exact: true })
          .fill("acme");
        await page
          .getByLabel(
            ja ? "メモリプロバイダのバージョン" : "Memory provider version",
          )
          .selectOption("memory@1.0.0");
      }
      await page
        .getByLabel(ja ? "記憶の検索・考察" : "Recall or reflect")
        .fill("Current evidence");
      for (const action of ["recall", "reflect"]) {
        const request = page.waitForRequest(
          (request) =>
            request.url().endsWith("/memory/operate") &&
            request.postDataJSON()?.action?.action === action,
        );
        await page
          .getByRole("button", {
            name:
              action === "recall"
                ? ja
                  ? "検索"
                  : "Recall"
                : ja
                  ? "根拠付きで考察"
                  : "Reflect with cited evidence",
            exact: true,
          })
          .click();
        expect((await request).postDataJSON().action.query.max_tokens).toBe(
          512,
        );
      }
    }
    expect(
      f.writes.filter((write) =>
        ["recall", "reflect"].includes(write.body.action?.action ?? ""),
      ),
    ).toHaveLength(4);
  });

  test(`memory Registry pins all six roles and an explicit graph cutoff (${locale})`, async ({
    page,
  }) => {
    await fixture(page, locale);
    const ja = locale === "ja-JP";
    await page.goto("/registry");
    await page
      .getByRole("button", {
        name: ja ? "エンティティを登録" : "Register entity",
        exact: true,
      })
      .click();
    const dialog = page.getByRole("dialog");
    await dialog.locator("select").first().selectOption("memory");
    await dialog
      .locator('[name="description_en"]')
      .fill("Current native memory / 最新のネイティブ記憶");
    for (const role of [
      "extraction",
      "derivation",
      "reflection",
      "embedding",
      "reranker",
      "tokenizer",
    ]) {
      const id = ["extraction", "derivation", "reflection"].includes(role)
        ? "model"
        : role;
      await dialog
        .locator(`[name="memory_role_${role}"]`)
        .selectOption(`${id}@1.0.0`);
    }
    for (const field of await dialog.locator('input[name^="price_"]').all())
      await field.fill("0");
    const cutoff = dialog.locator(
      '[name="semantic_link_min_similarity_millionths"]',
    );
    await cutoff.fill("0");
    expect(
      await cutoff.evaluate((element: HTMLInputElement) =>
        element.checkValidity(),
      ),
    ).toBe(false);
    await cutoff.fill("800000");
    await dialog.locator('[name="memory_learning"]').check();
    const request = page.waitForRequest(
      (request) =>
        request.method() === "POST" &&
        new URL(request.url()).pathname === "/api/registry",
    );
    await dialog
      .getByRole("button", {
        name: ja ? "エンティティを登録" : "Register entity",
        exact: true,
      })
      .click();
    const config = (await request).postDataJSON().config;
    expect(config.engine).toBe("hindsight_rust");
    expect(config.policy.semantic_link_min_similarity_millionths).toBe(800000);
    expect(config.policy.learn_from_runs).toBe(true);
    for (const role of [
      "extraction",
      "derivation",
      "reflection",
      "embedding",
      "reranker",
      "tokenizer",
    ]) {
      expect(config.policy[role]).toEqual(
        ref(
          ["extraction", "derivation", "reflection"].includes(role)
            ? "model"
            : role,
        ),
      );
    }
    expect(config.policy.bounds.max_model_calls).toBeGreaterThan(0);
    expect(config.policy.bounds.max_units ** 2 * 3072).toBeLessThanOrEqual(
      32 * 1024 * 1024,
    );
    expect(config.policy.retention.backup_days).toBe(7);
  });

  test(`native memory preserves structured provenance and retries the same request (${locale})`, async ({
    page,
  }) => {
    const f = await fixture(page, locale);
    const ja = locale === "ja-JP";
    await page
      .getByRole("button", {
        name: ja ? "Unitを追加" : "Add a memory unit",
        exact: true,
      })
      .click();
    const dialog = page.getByRole("dialog");
    await dialog
      .getByLabel(ja ? "内容" : "Content", { exact: true })
      .fill("東京の地下鉄。 Tokyo subway.");
    await dialog
      .getByRole("button", {
        name: ja ? "エンティティを追加" : "Add entity",
        exact: true,
      })
      .click();
    await dialog.getByLabel(ja ? "名前" : "Name", { exact: true }).fill("東京");
    await dialog
      .getByLabel(ja ? "分類" : "Category", { exact: true })
      .fill("city");
    await dialog
      .getByLabel(ja ? "別名（1行に1つ）" : "Aliases (one per line)")
      .fill("Tokyo\nEdo");
    await dialog
      .getByRole("button", {
        name: ja ? "正確な出典を追加" : "Add exact source",
        exact: true,
      })
      .click();
    await dialog
      .getByLabel(ja ? "出典ID" : "Source ID", { exact: true })
      .fill(source);
    await dialog.getByLabel("Revision", { exact: true }).fill("4");
    f.failNext();
    await dialog
      .getByRole("button", {
        name: ja ? "確認したrevisionを保存" : "Save the observed revision",
      })
      .click();
    await expect(dialog.getByRole("alert")).toContainText("temporary failure");
    await expect(
      dialog.getByRole("button", {
        name: ja ? "確認したrevisionを保存" : "Save the observed revision",
      }),
    ).toBeDisabled();
    await expect(
      page.getByLabel(ja ? "記憶の所有者" : "Memory bank"),
    ).toBeDisabled();
    await dialog
      .getByRole("button", {
        name: ja ? "同じ操作を再試行" : "Retry the same operation",
        exact: true,
      })
      .click();
    await expect.poll(() => f.writes.length).toBe(2);
    expect(f.writes[1]).toEqual(f.writes[0]);
    const saved = f.writes[0].body.changes![0].content;
    expect(saved.entities).toEqual([
      { name: "東京", category: "city", aliases: ["Tokyo", "Edo"] },
    ]);
    expect(saved.evidence).toEqual([
      {
        kind: "unit",
        id: source,
        revision: 4,
        bank: { home: "aidash://test", tenant: "acme", workspace, participant },
      },
    ]);
    expect(saved.verification).toBe("unverified");
    await expect(
      page.getByLabel(ja ? "記憶の所有者" : "Memory bank"),
    ).toBeEnabled();
  });

  test(`task assignment compares the currently recorded binding (${locale})`, async ({
    page,
  }) => {
    const f = await fixture(page, locale);
    const ja = locale === "ja-JP";
    await page
      .getByLabel(ja ? "タスク" : "Task", { exact: true })
      .selectOption(task);
    await page
      .getByRole("button", {
        name: ja
          ? "このAgentをTaskに割り当て"
          : "Assign this Agent to the Task",
        exact: true,
      })
      .click();
    await expect
      .poll(
        () =>
          f.writes.filter((write) => write.url.endsWith("/memory-participant"))
            .length,
      )
      .toBe(1);
    expect(
      f.writes.find((write) => write.url.endsWith("/memory-participant"))!.body,
    ).toEqual({
      task_revision: 7,
      expected: { participant_id: participant, participant_revision: 2 },
      target: { participant_id: participant, participant_revision: 3 },
    });
  });
}

for (const locale of ["en-US", "ja-JP"] as const) {
  test(`native memory pages history and impact with observed revision (${locale})`, async ({
    page,
  }) => {
    const f = await fixture(page, locale, true);
    const ja = locale === "ja-JP";
    await page
      .getByRole("button", { name: ja ? "履歴" : "History", exact: true })
      .click();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toContainText("Recent retained revision 31");
    await dialog
      .getByRole("button", {
        name: ja ? "以前のrevisionを表示" : "Older revisions",
        exact: true,
      })
      .click();
    await expect(dialog).toContainText("Earlier retained revision");
    const history = f.writes.filter(
      (write) => write.body.action?.action === "history",
    );
    expect(
      history.map((write) => write.body.action!.expected_revision),
    ).toEqual([40, 40]);
    expect(history.map((write) => write.body.action!.before)).toEqual([
      null,
      9,
    ]);
    await page.keyboard.press("Escape");
    await page
      .getByText(
        ja
          ? "鮮度・履歴・影響するRun・消去状況"
          : "Freshness, history, affected Runs and cleanup",
        { exact: true },
      )
      .click();
    await page
      .getByRole("button", {
        name: ja ? "さらにUnitの状態を表示" : "More unit status",
        exact: true,
      })
      .click();
    await expect(page).toHaveURL(/semantic/);
    const impact = page.locator("details").filter({
      has: page.getByText(
        ja
          ? "鮮度・履歴・影響するRun・消去状況"
          : "Freshness, history, affected Runs and cleanup",
        { exact: true },
      ),
    });
    await expect(impact).toContainText(ja ? "影響する Run" : "Affected Runs");
    await expect(impact).toContainText("23");
    expect(f.inspections.map((action) => action.after)).toEqual([null, source]);
    await page
      .getByRole("button", { name: ja ? "履歴" : "History", exact: true })
      .click();
    await expect(dialog).toContainText("Recent retained revision 31");
    const bankSelect = page.getByLabel(ja ? "記憶の所有者" : "Memory bank");
    await bankSelect.selectOption("");
    await expect(dialog).toHaveCount(0);
    await bankSelect.selectOption(participant);
    await expect(dialog).toHaveCount(0);
    await expect(page.getByText("Recent retained revision 31")).toHaveCount(0);
  });
  test(`native memory clears retained bodies after disclosure withdrawal (${locale})`, async ({
    page,
  }) => {
    const f = await fixture(page, locale, true);
    const ja = locale === "ja-JP";
    await page
      .getByRole("button", { name: ja ? "履歴" : "History", exact: true })
      .click();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toContainText("Recent retained revision 31");
    f.withdraw();
    await expect(dialog).not.toContainText("Recent retained revision 31", {
      timeout: 15000,
    });
    await expect(
      page.getByText("A history-bearing memory unit", { exact: true }),
    ).toHaveCount(0);
  });
}

for (const locale of ["en-US", "ja-JP"] as const) {
  test(`native memory visualizes content, provenance and model usage (${locale})`, async ({
    page,
  }) => {
    await fixture(page, locale, true, true);
    const ja = locale === "ja-JP";
    const unit = page.locator(`[id="memory-unit-${source}"]`);
    await expect(
      page.getByLabel(ja ? "記憶の概要" : "Memory overview"),
    ).toContainText("2");
    await expect(unit).toContainText(ja ? "未検証" : "Unverified");
    await expect(
      unit.getByRole("list", { name: ja ? "エンティティ" : "Entities" }),
    ).toContainText("Postgres · ポストグレス");
    await unit.locator("summary").click();
    const proof = unit.getByLabel(
      ja ? "根拠のつながり" : "Evidence connections",
    );
    await expect(proof).toContainText(
      ja ? "この記憶が引用" : "Cited by this memory",
    );
    await proof
      .getByRole("button", {
        name: "The team prefers PostgreSQL for durable memory.",
      })
      .click();
    await expect(
      page.locator("#memory-unit-019c0000-0000-7000-8000-000000000005"),
    ).toBeFocused();
    await expect(unit.locator("pre")).toHaveCount(0);
    if (process.env.AIDASH_MEMORY_SCREENSHOT)
      await page.screenshot({
        path: `/tmp/aidash-pr130-repair/native-memory-${locale}.png`,
        fullPage: true,
      });
    await page
      .getByLabel(ja ? "記憶の検索・考察" : "Recall or reflect")
      .fill("What do we know?");
    await page
      .getByRole("button", {
        name: ja ? "根拠付きで考察" : "Reflect with cited evidence",
        exact: true,
      })
      .click();
    const result = page.locator("details").filter({
      has: page.getByText(ja ? "操作結果" : "Operation result", {
        exact: true,
      }),
    });
    await expect(result).toContainText(
      "A readable reflection with cited evidence",
    );
    await result.locator("summary").last().click();
    await expect(result).toContainText("sha256:observed-run");
    await expect(result.locator("pre")).toHaveCount(0);
    await page
      .getByRole("button", {
        name: ja ? "モデル使用量・計上コスト" : "Model usage and charged cost",
        exact: true,
      })
      .click();
    await expect(result).toContainText(ja ? "トークン数" : "Tokens");
    await expect(result).toContainText("345");
    await expect(result).toContainText(
      ja ? "計上コスト（µUSD）" : "Charged cost (µUSD)",
    );
    await expect(result.locator("pre")).toHaveCount(0);
    await page
      .getByRole("button", { name: ja ? "検索" : "Recall", exact: true })
      .click();
    await expect(result).toContainText(
      "Recalled memory with readable evidence",
    );
    await page.setViewportSize({ width: 390, height: 844 });
    await expect
      .poll(() =>
        unit.evaluate((element) => element.scrollWidth <= element.clientWidth),
      )
      .toBe(true);
    if (process.env.AIDASH_MEMORY_SCREENSHOT)
      await unit.screenshot({
        path: `/tmp/aidash-pr130-repair/native-memory-mobile-${locale}.png`,
      });
  });
}
