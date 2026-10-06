import { test, expect } from "@playwright/test";
import { readFileSync, writeFileSync } from "node:fs";
import { installBearerDashboard } from "./auth-fixture";

// Authentication presentation uses the existing session shim. Every state,
// Registry and memory request reaches the real subject-authorized Home API.
test("review a real canonical Run candidate through the dashboard", async ({
  page,
}) => {
  const input = process.env.AIDASH_MEMORY_LIVE_INPUT;
  const output = process.env.AIDASH_MEMORY_LIVE_OUTPUT;
  test.skip(
    !input || !output,
    "Requires an explicitly provisioned live Home fixture",
  );
  const profile = JSON.parse(readFileSync(input!, "utf8"));
  const ja = profile.language === "ja-JP";
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await installBearerDashboard(page, profile.token, {
    tenant: "acme",
    name: "alice",
  });
  await page.addInitScript(
    (locale) => localStorage.setItem("aidash-locale", locale),
    profile.language,
  );
  await page.goto("/semantic");
  await page
    .locator(".semantic-page")
    .getByLabel(ja ? "ワークスペース" : "Workspace", { exact: true })
    .selectOption(profile.workspace);
  await page
    .getByLabel(ja ? "記憶の所有者" : "Memory bank", { exact: true })
    .selectOption(profile.bank.participant);
  const candidate = page
    .locator("article")
    .filter({ hasText: profile.candidate.content.text });
  await candidate
    .getByRole("button", {
      name: ja
        ? "内容を確認・編集して採用"
        : "Review and edit before admission",
      exact: true,
    })
    .click();
  const dialog = page.getByRole("dialog").filter({
    has: page.getByRole("heading", {
      name: ja ? "学習候補のレビュー" : "Review the candidate",
      exact: true,
    }),
  });
  await expect(
    dialog.getByLabel(ja ? "検証状態" : "Verification", { exact: true }),
  ).toHaveValue("unverified");
  const response = page.waitForResponse((response) => {
    if (
      !response.url().endsWith("/memory/operate") ||
      response.request().method() !== "POST"
    )
      return false;
    const body = response.request().postDataJSON();
    return (
      body.action.action === "review" && body.action.id === profile.candidate.id
    );
  });
  await dialog
    .getByRole("button", {
      name: ja ? "確認したrevisionを保存" : "Save the observed revision",
      exact: true,
    })
    .click();
  const actual = await response;
  expect(actual.status()).toBe(200);
  const request = actual.request().postDataJSON();
  const result = await actual.json();
  expect(request.bank).toEqual(profile.bank);
  expect(request.operation_id).toBe(request.action.mutation.operation_id);
  expect(request.action.expected_revision).toBe(profile.candidate.revision);
  expect(result.value.content).toEqual(profile.candidate.content);
  await expect(dialog).toHaveCount(0);
  expect(errors).toEqual([]);
  await page.screenshot({ path: output! + ".png", fullPage: true });
  writeFileSync(
    output!,
    JSON.stringify(
      {
        request,
        result,
        language: profile.language,
        scope:
          "Real memory/state/Registry HTTP with an authentication presentation shim",
      },
      null,
      2,
    ),
    { mode: 0o600 },
  );
});
