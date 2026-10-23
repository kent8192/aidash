import { test, expect, type Page } from "@playwright/test";
import { installBearerDashboard } from "./auth-fixture";
import {
  routeRegistryVersions,
  routeTenantActions,
  tenantSubject,
} from "./registry-fixture";

async function setup(page: Page, models?: Record<string, unknown>[]) {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await installBearerDashboard(page, "fixture-token");
  await page.addInitScript(() => {
    localStorage.setItem("aidash-locale", "ja-JP");
  });
  await page.route("**/api/**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path === "/api/events/stream") {
      await route.abort();
    } else if (path === "/api/session") {
      await route.fulfill({
        json: {
          access: tenantSubject,
          node_id: "aidash://test",
        },
      });
    } else if (path === "/api/state") {
      await route.fulfill({
        json: {
          access: tenantSubject,
          deployment: {
            id: "aidash://test",
            endpoint: "http://localhost",
            protocol_version: "0.1",
            key: null,
          },
          registry: [
            ...(models ?? [
              {
                id: "model",
                version: "1.0.0",
                kind: "model",
                name: { en: "Model" },
                description: { en: "Model" },
                capabilities: [],
                languages: [],
                tags: [],
                config: {},
              },
            ]),
            {
              id: "fixture-cluster",
              version: "1.0.0",
              kind: "cluster",
              name: { en: "Fixture cluster" },
              description: { en: "Coordination fixture" },
              config: {},
            },
            ...["1.0.0", "2.0.0"].map((version) => ({
              id: "research-skill",
              version,
              kind: "skill",
              name: { en: "Research skill" },
              description: { en: "Instructions" },
              capabilities: [],
              languages: [],
              tags: [],
              config: { instructions: "Research carefully" },
            })),
          ],
          workspaces: [],
          tasks: [],
          artifacts: [],
          events: [],
          runs: [],
          conversations: [],
          human_requests: [],
          installations: [],
        },
      });
    } else if (path === "/api/discover") {
      await route.fulfill({ json: { agents: [], errors: [] } });
    } else if (
      (path === "/api/registry" || path === "/api/agents/personal") &&
      route.request().method() === "POST"
    ) {
      await route.fulfill({
        json:
          path === "/api/agents/personal"
            ? route.request().postDataJSON().entry
            : route.request().postDataJSON(),
      });
    } else {
      await route.fulfill({ json: [] });
    }
  });
  await routeTenantActions(page, ["registry.create"], {
    deployment: "aidash://test",
  });
  await routeRegistryVersions(page, () => []);
  return errors;
}

test("registers an agent with selected versioned skills", async ({ page }) => {
  const errors = await setup(page);
  await page.goto("/registry");
  await page
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("エンティティの種類").selectOption("agent");
  await expect(dialog.getByLabel("エンティティID")).toHaveCount(0);
  await dialog.getByLabel("名前").fill("Skilled agent");
  await dialog.getByLabel("説明").fill("Uses selected skill versions");
  await dialog.locator('[name="model"]').selectOption("model@1.0.0");
  await expect(dialog.locator('[name="instructions"]')).not.toHaveAttribute(
    "required",
    "",
  );
  for (const name of ["skill_list", "skill_load", "skill_read"])
    await dialog.getByLabel(name, { exact: true }).uncheck();
  await dialog
    .getByLabel("追加するもの")
    .selectOption("skill:research-skill@2.0.0");
  await dialog
    .getByRole("button", { name: "このエージェントに追加", exact: true })
    .click();
  await expect(
    dialog.getByText("skill: aidash://test/research-skill@1.0.0", {
      exact: true,
    }),
  ).toHaveCount(0);
  for (const name of ["skill_list", "skill_load", "skill_read"]) {
    await expect(dialog.getByLabel(name, { exact: true })).toBeChecked();
    await expect(dialog.getByLabel(name, { exact: true })).toBeDisabled();
  }
  for (const name of ["task_create", "task_delegate", "agent_discover"])
    await dialog.getByLabel(name, { exact: true }).uncheck();
  await dialog
    .locator('[name="cluster"]')
    .selectOption("fixture-cluster@1.0.0");
  for (const name of ["task_create", "task_delegate", "agent_discover"]) {
    await expect(dialog.getByLabel(name, { exact: true })).toBeChecked();
    await expect(dialog.getByLabel(name, { exact: true })).toBeDisabled();
  }
  await dialog.locator('[name="cluster"]').selectOption("");
  await expect(
    dialog.getByLabel("task_delegate", { exact: true }),
  ).toBeEnabled();
  await expect(dialog.getByLabel("memory_write", { exact: true })).toHaveCount(
    0,
  );
  for (const name of ["memory_mutate", "memory_recall", "memory_reflect"])
    await dialog.getByLabel(name, { exact: true }).uncheck();
  const submitted = page.waitForRequest(
    (request) =>
      new URL(request.url()).pathname === "/api/registry" &&
      request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  expect((await submitted).postDataJSON().config).toMatchObject({
    model: { id: "model", version: "1.0.0" },
    schema_version: 1,
    bindings: [
      {
        kind: "skill",
        target: {
          registry_deployment: "aidash://test",
          id: "research-skill",
          version: "2.0.0",
        },
        narrow: {},
      },
    ],
    remove_default: ["memory_mutate", "memory_recall", "memory_reflect"],
    instructions: "",
  });
  await expect(dialog).not.toBeVisible();
  expect(errors).toEqual([]);
});

function pdfFixture(): Buffer {
  const stream = "BT /F1 12 Tf 72 720 Td (Private PDF reference) Tj ET";
  const objects = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>",
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    `<< /Length ${stream.length} >>\nstream\n${stream}\nendstream`,
  ];
  let pdf = "%PDF-1.4\n";
  const offsets = [0];
  objects.forEach((object, index) => {
    offsets.push(Buffer.byteLength(pdf));
    pdf += `${index + 1} 0 obj\n${object}\nendobj\n`;
  });
  const xref = Buffer.byteLength(pdf);
  pdf += `xref\n0 6\n0000000000 65535 f \n${offsets
    .slice(1)
    .map((n) => `${String(n).padStart(10, "0")} 00000 n \n`)
    .join("")}trailer\n<< /Size 6 /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF`;
  return Buffer.from(pdf);
}

test("uploads real PDF and Excel reference text separately from registry metadata", async ({
  page,
}) => {
  const errors = await setup(page);
  const ExcelJS = (await import("exceljs")).default;
  const workbook = new ExcelJS.Workbook();
  const sheet = workbook.addWorksheet("Budget");
  sheet.getCell("A1").value = "Private Excel reference";
  sheet.getCell("B2").value = 1234;
  await page.goto("/registry");
  await page
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("エンティティの種類").selectOption("agent");
  await dialog.getByLabel("名前").fill("Personal agent");
  await dialog.getByLabel("説明").fill("Personal documents");
  await dialog.locator('[name="model"]').selectOption("model@1.0.0");
  await dialog
    .getByLabel("追加するもの")
    .selectOption("skill:research-skill@2.0.0");
  await dialog
    .getByRole("button", { name: "このエージェントに追加", exact: true })
    .click();
  await dialog.getByLabel("参考資料を追加").setInputFiles([
    { name: "private.pdf", mimeType: "application/pdf", buffer: pdfFixture() },
    {
      name: "budget.xlsx",
      mimeType:
        "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
      buffer: Buffer.from(await workbook.xlsx.writeBuffer()),
    },
  ]);
  await expect(dialog.getByText("private.pdf", { exact: true })).toBeVisible();
  await expect(dialog.getByText("budget.xlsx", { exact: true })).toBeVisible();
  await dialog.getByText("private.pdf", { exact: true }).click();
  await expect(dialog.getByText(/Private PDF reference/)).toBeVisible();
  const sent = page.waitForRequest(
    (r) => new URL(r.url()).pathname === "/api/agents/personal",
  );
  await dialog
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  const input = (await sent).postDataJSON();
  expect(input.documents).toHaveLength(2);
  expect(input.documents[0].text).toContain("Private PDF reference");
  expect(input.documents[1].text).toContain("[Sheet: Budget]");
  expect(input.documents[1].text).toContain("B2: 1234");
  expect(JSON.stringify(input.entry)).not.toContain("Private Excel reference");
  await expect(dialog).not.toBeVisible();
  expect(errors).toEqual([]);
});

test("imports a published SKILL.md and rejects malformed frontmatter", async ({
  page,
}) => {
  await setup(page);
  await page.goto("/registry");
  await page
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("エンティティの種類").selectOption("skill");
  await dialog.getByLabel("SKILL.md", { exact: true }).setInputFiles({
    name: "SKILL.md",
    mimeType: "text/markdown",
    buffer: Buffer.from("---\nname: Invalid Name\n---\nInstructions"),
  });
  await expect(dialog.getByRole("alert")).toBeVisible();
  const skill =
    "---\nname: research\ndescription: Research primary sources\nlicense: MIT\n---\nCheck evidence before drawing conclusions.\n";
  await dialog.getByLabel("SKILL.md", { exact: true }).setInputFiles({
    name: "SKILL.md",
    mimeType: "text/markdown",
    buffer: Buffer.from(skill),
  });
  await expect(dialog.getByLabel("指示", { exact: true })).toHaveValue(skill);
  await expect(dialog.getByRole("alert")).toHaveCount(0);
});

test("imports a GitHub Skill directory with reference files", async ({
  page,
}) => {
  const errors = await setup(page);
  const skill =
    "---\nname: research\ndescription: Research primary sources\n---\nRead references/method.md when needed.\n";
  await page.route("**/api/skills/import", async (route) => {
    const request = route.request().postDataJSON();
    expect(request.url).toBe("https://github.com/example/skills");
    await route.fulfill({
      json: {
        skills: ["skills/research/SKILL.md", "skills/write/SKILL.md"],
        selected: request.skill_path
          ? {
              path: "skills/research/SKILL.md",
              source:
                "https://github.com/example/skills/blob/abc123/skills/research/SKILL.md",
              instructions: skill,
              files: [
                {
                  path: "references/method.md",
                  content: "Compare primary sources.",
                },
                {
                  path: "assets/chart.png",
                  content: "AAEC",
                  encoding: "base64",
                },
              ],
            }
          : null,
      },
    });
  });
  await page.goto("/registry");
  await page
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("エンティティの種類").selectOption("skill");
  await dialog
    .getByLabel("Skillの取得元URL")
    .fill("https://github.com/example/skills");
  await dialog.getByRole("button", { name: "URLからSkillを探す" }).click();
  await dialog
    .getByLabel("リポジトリ内のSkill")
    .selectOption("skills/research/SKILL.md");
  await dialog.getByRole("button", { name: "選択したSkillを取り込む" }).click();
  await expect(dialog.getByLabel("指示", { exact: true })).toHaveValue(skill);
  await dialog.getByText("取り込んだファイル (2)").click();
  await dialog.getByText("references/method.md", { exact: true }).click();
  await expect(dialog.getByText("Compare primary sources.")).toBeVisible();
  await dialog.getByText("assets/chart.png", { exact: true }).click();
  await expect(
    dialog.getByText("バイナリファイルはbase64で保存されます。"),
  ).toBeVisible();
  await dialog.getByLabel("名前").fill("Research");
  await dialog.getByLabel("説明").fill("Research primary sources");
  const submitted = page.waitForRequest(
    (request) =>
      new URL(request.url()).pathname === "/api/registry" &&
      request.method() === "POST",
  );
  await dialog
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  expect((await submitted).postDataJSON().config).toMatchObject({
    instructions: skill,
    files: [
      { path: "references/method.md", content: "Compare primary sources." },
      { path: "assets/chart.png", content: "AAEC", encoding: "base64" },
    ],
    source:
      "https://github.com/example/skills/blob/abc123/skills/research/SKILL.md",
  });
  expect(errors).toEqual([]);
});

test("clears an imported Skill while choosing from another repository", async ({
  page,
}) => {
  const errors = await setup(page);
  const skill =
    "---\nname: research\ndescription: Research sources\n---\nRead references/method.md.\n";
  await page.route("**/api/skills/import", async (route) => {
    const { url } = route.request().postDataJSON();
    await route.fulfill({
      json: url.endsWith("/first")
        ? {
            skills: ["SKILL.md"],
            selected: {
              path: "SKILL.md",
              source: `${url}/blob/abc123/SKILL.md`,
              instructions: skill,
              files: [
                { path: "references/method.md", content: "First repository." },
              ],
            },
          }
        : {
            skills: ["skills/one/SKILL.md", "skills/two/SKILL.md"],
            selected: null,
          },
    });
  });
  await page.goto("/registry");
  await page
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("エンティティの種類").selectOption("skill");
  await dialog
    .getByLabel("Skillの取得元URL")
    .fill("https://github.com/example/first");
  await dialog.getByRole("button", { name: "URLからSkillを探す" }).click();
  await expect(dialog.getByLabel("指示", { exact: true })).toHaveValue(skill);
  await dialog
    .getByLabel("Skillの取得元URL")
    .fill("https://github.com/example/second");
  await expect(dialog.getByLabel("指示", { exact: true })).toHaveValue("");
  await expect(dialog.getByText("取り込んだファイル (1)")).toHaveCount(0);
  await dialog.getByRole("button", { name: "URLからSkillを探す" }).click();
  await expect(dialog.getByLabel("リポジトリ内のSkill")).toBeVisible();
  await expect(dialog.getByLabel("指示", { exact: true })).toHaveValue("");
  expect(errors).toEqual([]);
});

test("imports a skills.sh registry page and its supporting files", async ({
  page,
}) => {
  const errors = await setup(page);
  const skill =
    "---\nname: editor\ndescription: Edit copy\n---\nRead references/style.md.\n";
  await page.route("**/api/skills/import", async (route) => {
    expect(route.request().postDataJSON().url).toBe(
      "https://skills.sh/example/skills/editor",
    );
    await route.fulfill({
      json: {
        skills: ["SKILL.md"],
        selected: {
          path: "SKILL.md",
          source: "https://skills.sh/example/skills/editor",
          instructions: skill,
          files: [
            { path: "references/style.md", content: "Use clear language." },
          ],
        },
      },
    });
  });
  await page.goto("/registry");
  await page
    .getByRole("button", { name: "エンティティを登録", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("エンティティの種類").selectOption("skill");
  await dialog
    .getByLabel("Skillの取得元URL")
    .fill("https://skills.sh/example/skills/editor");
  await dialog.getByRole("button", { name: "URLからSkillを探す" }).click();
  await expect(dialog.getByLabel("指示", { exact: true })).toHaveValue(skill);
  await dialog.getByText("取り込んだファイル (1)").click();
  await dialog.getByText("references/style.md", { exact: true }).click();
  await expect(dialog.getByText("Use clear language.")).toBeVisible();
  expect(errors).toEqual([]);
});

for (const selected of [
  {
    source: "provider_credential:openrouter",
    id: "tenant-model",
    label: "Tenant のプロバイダ認証情報",
  },
  {
    source: "plan_connection:chatgpt",
    id: "plan-model",
    label: "個人 Plan 接続",
  },
  { source: "keyless:openai", id: "local-model", label: "Keyless" },
])
  test(`agent selection cascades ${selected.source} to its versioned Registry reference`, async ({
    page,
  }) => {
    await setup(
      page,
      [
        {
          id: "tenant-model",
          config: { provider: "openrouter", provider_credential: "openrouter" },
        },
        {
          id: "plan-model",
          config: { provider: "chatgpt", plan_connection: "chatgpt" },
        },
        {
          id: "local-model",
          config: {
            provider: "openrouter",
            endpoint: "http://localhost:11434/v1",
          },
        },
      ].map((entry) => ({
        ...entry,
        kind: "model",
        version: "2.1.0",
        name: { en: entry.id },
        description: { en: entry.id },
        capabilities: [],
        tags: [],
        languages: [],
        skills: [],
        schema: {},
      })),
    );
    await page.route("**/api/tenants/acme/plan-connections/chatgpt", (route) =>
      route.fulfill({ json: { enabled: true, connection: null } }),
    );
    await page.goto("/registry");
    await page
      .getByRole("button", { name: "エンティティを登録", exact: true })
      .click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("名前").fill("Source-first agent");
    await dialog
      .getByLabel("説明")
      .fill("Keeps a versioned Registry model reference");
    await dialog
      .locator('[name="instructions"]')
      .fill("Follow the task instructions carefully.");
    const provider = dialog.getByLabel("プロバイダー", { exact: true });
    const model = dialog.locator('[name="model"]');
    await expect(model).toBeDisabled();
    await provider.selectOption("plan_connection:chatgpt");
    await model.selectOption("plan-model@2.1.0");
    await expect(dialog.getByText("未接続", { exact: true })).toBeVisible();
    await expect(
      dialog.getByRole("link", { name: "ChatGPT Plan の個人設定" }),
    ).toHaveAttribute("href", "/settings?view=planConnections");
    await provider.selectOption(selected.source);
    if (selected.source !== "plan_connection:chatgpt")
      await expect(model).toHaveValue("");
    await expect(
      provider.locator(`option[value="${selected.source}"]`),
    ).toContainText(selected.label);
    await expect(model.locator('option:not([value=""])')).toHaveCount(1);
    await model.selectOption(`${selected.id}@2.1.0`);
    const sent = page.waitForRequest(
      (request) =>
        new URL(request.url()).pathname === "/api/registry" &&
        request.method() === "POST",
    );
    await dialog
      .getByRole("button", { name: "エンティティを登録", exact: true })
      .click();
    expect((await sent).postDataJSON().config.model).toEqual({
      id: selected.id,
      version: "2.1.0",
    });
  });
