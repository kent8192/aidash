import { expect, test } from "@playwright/test";
import { setup } from "./collaboration-fixture";

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
      model: { id: ids[0], version: "9.0.0" },
      tools: [{ id: ids[1], version: "1.0.0" }],
    },
  },
  entry(5, "model", "Fallback model", ""),
  entry(6, "model", "", ""),
  entry(7, "model", "Research model", "調査モデル"),
];

for (const locale of ["en-US", "ja-JP"]) {
  test(`registry labels use names while submitting IDs in ${locale}`, async ({
    page,
  }) => {
    const { errors } = await setup(page);
    await page.addInitScript(
      (locale) => localStorage.setItem("aidash-locale", locale),
      locale,
    );
    await page.route("**/api/state", (route) =>
      route.fulfill({
        json: {
          access: { kind: "operator" },
          node: {
            id: "aidash://home",
            endpoint: "http://localhost",
            protocol_version: "0.1",
            capabilities: [],
            clusters: [],
          },
          registry,
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
    await dialog
      .getByLabel(`${japanese ? "調査スキル" : "Research skill"} · 1.0.0`, {
        exact: true,
      })
      .check();
    await dialog
      .getByLabel(`${japanese ? "検索ツール" : "Search tool"} · 1.0.0`, {
        exact: true,
      })
      .check();
    await dialog.locator('select[name="cluster"]').selectOption({
      label: `${japanese ? "調査チーム" : "Research team"} · 1.0.0`,
    });
    for (const id of ids) await expect(dialog).not.toContainText(id);
    await dialog.getByRole("button", { name: register, exact: true }).click();
    await expect
      .poll(() => submitted?.config)
      .toMatchObject({
        model: { id: ids[0], version: "2.0.0" },
        tools: [{ id: ids[1], version: "1.0.0" }],
        skills: [{ id: ids[2], version: "1.0.0" }],
        cluster: { id: ids[3], version: "1.0.0" },
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
