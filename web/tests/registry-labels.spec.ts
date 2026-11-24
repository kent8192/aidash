import { expect, test } from "@playwright/test";
import { fixture, setup } from "./collaboration-fixture";
import {
  registryVersion,
  routeRegistryVersions,
  tenantSubject,
} from "./registry-fixture";

const ids = Array.from(
  { length: 8 },
  (_, index) =>
    `019a0000-0000-7000-8000-${String(index + 1).padStart(12, "0")}`,
);
const entry = (
  index: number,
  kind: string,
  en: string,
  ja = en,
  version = "1.0.0",
) => ({
  id: ids[index],
  version,
  kind,
  name: { en, ja },
  description: { en: "Fixture" },
  config: {},
  schema: {},
  capabilities: [],
  languages: ["en", "ja"],
  tags: [],
  skills: [],
});
const registry = [
  entry(0, "model", "Research model", "調査モデル"),
  entry(0, "model", "Research model", "調査モデル", "2.0.0"),
  entry(1, "tool", "Search tool", "検索ツール"),
  entry(2, "skill", "Research skill", "調査スキル"),
  entry(3, "cluster", "Research team", "調査チーム"),
  {
    ...entry(4, "agent", "Researcher", "調査担当"),
    config: {
      schema_version: 1,
      model: { id: ids[0], version: "9.0.0" },
      bindings: [
        {
          kind: "tool",
          target: {
            registry_deployment: "aidash://home",
            id: ids[1],
            version: "1.0.0",
          },
          narrow: {},
        },
      ],
      remove_default: [],
    },
  },
  entry(5, "model", "Fallback model", ""),
  entry(6, "model", "", ""),
  entry(7, "model", "Research model", "調査モデル"),
];

test("Open Source details offer revoke when Registry versions become forbidden", async ({
  page,
}) => {
  await page.clock.install();
  await setup(page, { subject: true });
  const allowed = {
    reference_id: "019b0000-0000-7000-8000-000000000001",
    path: "allowed.md",
  };
  const denied = {
    reference_id: "019b0000-0000-7000-8000-000000000002",
    path: "denied.md",
  };
  const source = {
    ...entry(0, "source", "Project references"),
    config: {
      schema_version: 1,
      source: {
        adapter: "reference_attachments",
        references: [allowed, denied].map((file) => ({
          ...file,
          digest: "a".repeat(64),
        })),
      },
    },
  };
  await page.route("**/api/state**", (route) =>
    route.fulfill({
      json: { ...fixture(), access: tenantSubject, registry: [source] },
    }),
  );
  let mayReadCollection = true;
  let mayRevoke = false;
  const hints = () => [
    mayRevoke
      ? { ...allowed, may_revoke: true, revision: 7 }
      : { ...allowed, may_revoke: false },
    { ...denied, may_revoke: false },
  ];
  await page.route(
    (url) => url.pathname === "/api/registry/versions",
    (route) =>
      mayReadCollection
        ? route.fulfill({
            json: {
              versions: [
                registryVersion(source, { reference_revocations: hints() }),
              ],
              next_offset: null,
            },
          })
        : route.fulfill({ status: 403, json: {} }),
  );
  const hintsPath = `/api/registry/${source.id}/${source.version}/reference-revocations`;
  await page.route(
    (url) => url.pathname === hintsPath,
    (route) => route.fulfill({ json: hints() }),
  );
  const probes: string[] = [];
  await page.route("**/api/references/*/revocation", (route) => {
    probes.push(route.request().url());
    return route.fulfill({ status: 404, json: {} });
  });
  const revokePath = `**/api/references/${allowed.reference_id}/revoke`;
  await page.route(revokePath, (route) => {
    mayRevoke = false;
    return route.fulfill({
      json: { reference_id: allowed.reference_id, status: "revoked" },
    });
  });

  await page.goto("/settings?view=registry");
  await page.getByRole("button", { name: /Project references/ }).click();
  const dialog = page.getByRole("dialog");
  await expect(
    dialog.getByRole("heading", { name: "Project references", exact: true }),
  ).toBeVisible();
  const allowedRow = dialog
    .getByRole("listitem")
    .filter({ hasText: "allowed.md" });
  const deniedRow = dialog
    .getByRole("listitem")
    .filter({ hasText: "denied.md" });
  await expect(deniedRow).toContainText("Cannot revoke");
  await expect(allowedRow).toContainText("Cannot revoke");
  expect(probes).toEqual([]);
  mayReadCollection = false;
  mayRevoke = true;
  const collectionDenied = page.waitForResponse(
    (response) => new URL(response.url()).pathname === "/api/registry/versions",
  );
  await page.clock.fastForward(2000);
  await page.evaluate(() =>
    window.dispatchEvent(new Event("visibilitychange")),
  );
  expect((await collectionDenied).status()).toBe(403);
  await expect(
    dialog.getByRole("button", { name: "Revoke", exact: true }),
  ).toHaveCount(1);
  await allowedRow.getByRole("button", { name: "Revoke", exact: true }).click();
  const submitted = page.waitForRequest(revokePath);
  const refreshed = page.waitForResponse(
    (response) => new URL(response.url()).pathname === hintsPath,
  );
  await allowedRow.getByRole("button", { name: "Confirm revoke" }).click();
  expect((await submitted).postDataJSON()).toEqual({ expected_revision: 7 });
  expect((await refreshed).ok()).toBe(true);
  await expect(allowedRow).toContainText("Cannot revoke");
  expect(probes).toEqual([]);
});

for (const locale of ["en-US", "ja-JP"]) {
  test(`registry labels use names while submitting IDs in ${locale}`, async ({
    page,
  }) => {
    const { errors } = await setup(page, { subject: true });
    await page.addInitScript(
      (locale) => localStorage.setItem("aidash-locale", locale),
      locale,
    );
    await routeRegistryVersions(page, () =>
      registry.map((entry) => registryVersion(entry)),
    );
    await page.route("**/api/state**", (route) =>
      route.fulfill({
        json: {
          access: tenantSubject,
          deployment: {
            id: "aidash://home",
            endpoint: "http://localhost",
            protocol_version: "0.2",
            key: null,
          },
          registry,
          threads: [],
          tasks: [],
          artifacts: [],
          events: [],
          runs: [],
          conversations: [],
          next_cursors: {
            threads: null,
            tasks: null,
            runs: null,
            artifacts: null,
            human_requests: null,
            conversations: null,
          },
          human_requests: [],
          installations: [],
        },
      }),
    );
    let submitted: Record<string, unknown> | undefined;
    await page.route("**/api/registry", async (route) => {
      submitted = route.request().postDataJSON();
      await route.fulfill({ json: submitted });
    });
    const japanese = locale === "ja-JP";
    const register = japanese ? "エンティティを登録" : "Register entity";
    await page.goto("/settings?view=registry");
    await page.locator('[data-registry-kind="all"]').click();
    for (const id of ids)
      await expect(page.locator("body")).not.toContainText(id);
    await page.getByRole("button", { name: register, exact: true }).click();
    const dialog = page.getByRole("dialog");
    const model = dialog.locator('select[name="model"]');
    await expect(model.locator("option")).toHaveText([
      japanese ? "選択してください…" : "Choose…",
      `${japanese ? "調査モデル" : "Research model"} · 1.0.0 (#1)`,
      `${japanese ? "調査モデル" : "Research model"} · 2.0.0`,
      "Fallback model · 1.0.0",
      `${japanese ? "名称未設定の項目" : "Unnamed item"} · 1.0.0`,
      `${japanese ? "調査モデル" : "Research model"} · 1.0.0 (#2)`,
    ]);
    await model.selectOption({
      label: `${japanese ? "調査モデル" : "Research model"} · 1.0.0 (#2)`,
    });
    await expect(model).toHaveValue(`${ids[7]}@1.0.0`);
    await expect(
      page.locator(".entity-card h3").filter({
        hasText: `${japanese ? "調査モデル" : "Research model"} (#1)`,
      }),
    ).toHaveCount(1);
    await model.selectOption({
      label: `${japanese ? "調査モデル" : "Research model"} · 2.0.0`,
    });
    await dialog
      .getByLabel(japanese ? "名前" : "Name", { exact: true })
      .fill("New researcher");
    await dialog
      .getByLabel(japanese ? "説明" : "Description", { exact: true })
      .fill("Uses named references");
    const binding = dialog.getByLabel(
      japanese ? "追加するもの" : "Item to add",
    );
    const add = dialog.getByRole("button", {
      name: japanese ? "このエージェントに追加" : "Add to this Agent",
      exact: true,
    });
    await binding.selectOption(`skill:${ids[2]}@1.0.0`);
    await add.click();
    await binding.selectOption(`tool:${ids[1]}@1.0.0`);
    await add.click();
    for (const id of ids) await expect(dialog).not.toContainText(id);
    await dialog.getByRole("button", { name: register, exact: true }).click();
    await expect
      .poll(() => submitted?.config)
      .toMatchObject({
        model: { id: ids[0], version: "2.0.0" },
        schema_version: 1,
        bindings: [
          {
            kind: "skill",
            target: {
              registry_deployment: "aidash://home",
              id: ids[2],
              version: "1.0.0",
            },
            narrow: {},
          },
          {
            kind: "tool",
            target: {
              registry_deployment: "aidash://home",
              id: ids[1],
              version: "1.0.0",
            },
            narrow: {},
          },
        ],
      });
    await expect(dialog).toHaveCount(0);
    await page
      .locator(".entity-card")
      .filter({
        has: page.getByRole("heading", {
          name: japanese ? "調査担当" : "Researcher",
          exact: true,
        }),
      })
      .click();
    const metadata = page.getByRole("dialog").locator(".json");
    await expect(metadata).toContainText(
      `${japanese ? "参照できない項目" : "Unavailable item"} · 9.0.0`,
    );
    await expect(metadata).toContainText(
      `${japanese ? "検索ツール" : "Search tool"} · 1.0.0`,
    );
    for (const id of ids)
      await expect(page.getByRole("dialog")).not.toContainText(id);
    expect(errors).toEqual([]);
  });
}

test("embedding registration defaults to full-dimensional Gemini through OpenRouter", async ({
  page,
}) => {
  const { errors } = await setup(page, { subject: true });
  let submitted: Record<string, unknown> | undefined;
  await page.route("**/api/registry", async (route) => {
    submitted = route.request().postDataJSON();
    await route.fulfill({ json: submitted });
  });
  await page.goto("/settings?view=registry");
  await page
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("Entity type").selectOption("embedding");
  await expect(dialog.locator('[name="embedding_provider"]')).toHaveValue(
    "openrouter",
  );
  await expect(dialog.locator('[name="endpoint"]')).toHaveValue(
    "https://openrouter.ai/api/v1",
  );
  await expect(dialog.locator('[name="model_id"]')).toHaveValue(
    "google/gemini-embedding-2",
  );
  await expect(dialog.locator('[name="dimensions"]')).toHaveValue("3072");
  await dialog
    .getByLabel("Description", { exact: true })
    .fill("Multilingual semantic memory");
  await dialog
    .getByRole("button", { name: "Register entity", exact: true })
    .click();
  await expect
    .poll(() => submitted?.config)
    .toEqual({
      provider: "openrouter",
      endpoint: "https://openrouter.ai/api/v1",
      provider_credential: "openrouter",
      model: "google/gemini-embedding-2",
      model_version: "1.0.0",
      dimensions: 3072,
      inputs: ["text", "image"],
    });
  await expect(dialog).toHaveCount(0);
  expect(errors).toEqual([]);
});
