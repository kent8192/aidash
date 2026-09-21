import { test, expect } from "@playwright/test";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { randomUUID } from "node:crypto";

test("subject dashboard completes a conversation and clears revoked access", async ({
  page,
  request,
}) => {
  const provider = createServer(async (req, res) => {
    const chunks: Buffer[] = [];
    for await (const chunk of req) chunks.push(Buffer.from(chunk));
    const body = JSON.parse(Buffer.concat(chunks).toString());
    const context = JSON.parse(body.messages[1].content);
    const answered = context.history.some(
      (entry: { kind: string }) => entry.kind === "human",
    );
    const message = answered
      ? { role: "assistant", content: "Scoped task completed" }
      : {
          role: "assistant",
          content: null,
          tool_calls: [
            {
              id: "ask",
              type: "function",
              function: {
                name: "human_request",
                arguments: JSON.stringify({
                  kind: "QUESTION",
                  prompt: "Scoped confirmation",
                }),
              },
            },
          ],
        };
    res.writeHead(200, { "content-type": "application/json" });
    res.end(
      JSON.stringify({
        choices: [
          {
            index: 0,
            finish_reason: answered ? "stop" : "tool_calls",
            message,
          },
        ],
        usage: { prompt_tokens: 1, completion_tokens: 1 },
      }),
    );
  });
  await new Promise<void>((resolve) =>
    provider.listen(0, "127.0.0.1", resolve),
  );
  try {
    const api = async (path: string, body?: unknown) => {
      const options = {
        headers: { authorization: "Bearer acceptance-access-token" },
      };
      const response =
        body === undefined
          ? await request.get(path, options)
          : await request.post(path, { ...options, data: body });
      expect(response.status(), await response.text()).toBe(200);
      return response.json();
    };
    const id = `scoped-${randomUUID()}`;
    const tenant = `tenant-${id}`;
    const state = await api("/api/state");
    for (const [kind, entryId, config] of [
      [
        "model",
        `${id}-model`,
        {
          provider: "openrouter",
          model_id: "fixture",
          endpoint: `http://127.0.0.1:${(provider.address() as AddressInfo).port}/v1`,
          context_window: 128000,
          modalities: ["text"],
          cost: {},
        },
      ],
      [
        "agent",
        id,
        {
          model: { id: `${id}-model`, version: "1.0.0" },
          instructions: "Ask the human once, then complete the task.",
          tools: [],
          skills: [],
          max_steps: 8,
        },
      ],
    ] as const) {
      await api("/api/registry", {
        kind,
        id: entryId,
        version: "1.0.0",
        name: { en: entryId },
        description: { en: "Scoped browser fixture" },
        schema: { type: "object" },
        config,
      });
    }
    await api(`/api/authorization/${tenant}`, {
      expected_revision: 0,
      bundle: {
        tenant,
        subjects: {
          alice: { kind: "user" },
          [`${state.node.id}/agents/${id}@1.0.0`]: { kind: "agent" },
        },
        policies: [
          {
            id: "approved-work",
            effect: "allow",
            subjects: { any: true },
            actions: ["*"],
            resources: { kinds: ["*"] },
          },
        ],
      },
    });
    for (const entryId of [id, `${id}-model`]) {
      await api(`/api/authorization/${tenant}/catalog`, {
        entry: { id: entryId, version: "1.0.0" },
        expected_revision: 0,
        enabled: true,
      });
    }
    const credential = await api(`/api/authorization/${tenant}/credentials`, {
      subject: "alice",
    });
    const administrativeRequests: string[] = [];
    const errors: string[] = [];
    page.on("pageerror", (error) => errors.push(error.message));
    page.on("request", (req) => {
      if (/\/api\/(mesh|marketplace)(?:\?|$)/.test(req.url()))
        administrativeRequests.push(req.url());
    });
    await page.goto("/");
    await page.getByLabel("アクセストークン").fill(credential.token);
    await page.getByRole("button", { name: "接続", exact: true }).click();
    await expect(page.getByRole("heading", { name: "概要." })).toBeVisible();
    await expect(
      page
        .locator(".sidebar")
        .getByRole("link", { name: "メッシュ", exact: true }),
    ).toHaveCount(0);
    await expect(
      page
        .locator(".sidebar")
        .getByRole("link", { name: "マーケットプレイス", exact: true }),
    ).toHaveCount(0);
    await page
      .locator(".page-heading")
      .getByRole("button", { name: "新しいゴール" })
      .click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("タイトル", { exact: true }).fill(id);
    await dialog
      .getByLabel("ゴール", { exact: true })
      .fill("Complete a tenant conversation");
    await dialog.getByLabel("コーディネーター").selectOption(`${id}@1.0.0`);
    await dialog
      .getByRole("button", { name: "新しいゴール", exact: true })
      .click();
    await expect(dialog).toHaveCount(0);
    await page
      .locator(".request-card")
      .filter({ hasText: "Scoped confirmation" })
      .click({ timeout: 15000 });
    await dialog.getByLabel("回答", { exact: true }).fill("Confirmed");
    await dialog.getByRole("button", { name: "送信", exact: true }).click();
    await expect(dialog).toHaveCount(0);
    await expect
      .poll(
        async () => {
          const response = await request.get("/api/state", {
            headers: { authorization: `Bearer ${credential.token}` },
          });
          const state = await response.json();
          return state.tasks.find(
            (task: { title: string }) => task.title === id,
          )?.status;
        },
        { timeout: 15000 },
      )
      .toBe("COMPLETED");
    const response = await request.get("/api/state", {
      headers: { authorization: `Bearer ${credential.token}` },
    });
    const scoped = await response.json();
    expect(scoped.access).toEqual({
      kind: "subject",
      tenant,
      subject: "alice",
    });
    expect(scoped.conversations[0].created_by).toBe("alice");
    expect(scoped.human_requests[0].answered_by).toBe("alice");
    expect(scoped.artifacts).toHaveLength(1);
    await page
      .locator(".sidebar")
      .getByRole("link", { name: "設定", exact: true })
      .click();
    await expect(page.getByText(tenant, { exact: true })).toBeVisible();
    await expect(page.getByRole("button", { name: "Peerを追加" })).toHaveCount(
      0,
    );
    await page.getByLabel("言語").selectOption("en-US");
    await expect(page.getByText("Signed in as", { exact: true })).toBeVisible();
    await page.screenshot({
      path: "../.ignore/dashboard-scoped-access.png",
      fullPage: true,
    });
    await api(
      `/api/authorization/${tenant}/credentials/${credential.credential.id}/revoke`,
      {},
    );
    await expect(page.getByLabel("Access token", { exact: true })).toBeVisible({
      timeout: 15000,
    });
    await expect(page.getByRole("alert")).toContainText("expired or revoked");
    expect(
      await page.evaluate(() => sessionStorage.getItem("aidash-token")),
    ).toBeNull();
    expect(administrativeRequests).toEqual([]);
    expect(errors).toEqual([]);
  } finally {
    provider.closeAllConnections();
    await new Promise<void>((resolve, reject) =>
      provider.close((error) => (error ? reject(error) : resolve())),
    );
  }
});
