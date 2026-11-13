import { expect, test } from "@playwright/test";
import { setup, fixture } from "./collaboration-fixture";
import { defaultNode } from "./registry-fixture";
import type { ChannelMessagePage } from "../src/generated/models";

for (const locale of ["en-US", "ja-JP"] as const) {
  for (const { native, failure } of [
    { native: false, failure: "none" },
    { native: true, failure: "transport" },
    { native: true, failure: "create" },
    { native: true, failure: "activate" },
  ]) {
    test(`remote assignment pins the selected ${native ? "native" : "semantic"} memory providers after ${failure} in ${locale}`, async ({
      page,
    }) => {
      const ja = locale === "ja-JP";
      const { errors } = await setup(page, {
        subject: true,
        locale,
        openTask: true,
        remoteAssignment: true,
        nativeMemory: native,
      });
      const grants: Record<string, unknown>[] = [];
      let attempts = 0;
      await page.route("**/api/tasks/task-0/remote-grants**", async (route) => {
        if (new URL(route.request().url()).pathname.endsWith("/inspect"))
          return route.fulfill({
            json: { native_required: native, memory_available: true },
          });
        if (
          new URL(route.request().url()).pathname.endsWith("/remote-grants")
        ) {
          grants.push(route.request().postDataJSON());
          if (failure === "create" && grants.length === 1)
            return route.fulfill({
              status: 409,
              json: { error: { message: "Participant changed" } },
            });
          return route.fulfill({ json: { id: grants.at(-1)!.id } });
        }
        if (native && ++attempts === 1) {
          if (failure === "transport") return route.abort("failed");
          if (failure === "activate")
            return route.fulfill({
              status: 422,
              json: { error: { message: "Grant rejected" } },
            });
        }
        return route.fulfill({
          json: {
            admission_id: "remote-run",
            phase: "RECEIVED",
            control: "ACTIVE",
          },
        });
      });
      await page.goto("/collaboration?channel=workspace-one");
      await page
        .getByRole("button", {
          name: ja ? "タスクと成果物" : "Tasks and results",
          exact: true,
        })
        .click();
      await page
        .getByRole("dialog")
        .getByRole("button")
        .filter({ hasText: "Collect evidence" })
        .click();
      await page
        .getByRole("button", {
          name: ja ? "担当を割り当て" : "Assign agent",
          exact: true,
        })
        .click();
      const dialog = page.getByRole("dialog");
      await dialog
        .locator("select[name=agent]")
        .selectOption(
          JSON.stringify(["aidash://remote", "researcher", "1.0.0"]),
        );
      const enabled = dialog.getByLabel(
        ja
          ? "各推論の前に Home の記憶を検索する"
          : "Require Home memory before each inference",
      );
      if (native) {
        await expect(enabled).toBeChecked();
        await expect(enabled).toBeDisabled();
        await dialog
          .getByLabel(ja ? "Homeの論理Agent" : "Home logical Agent", {
            exact: true,
          })
          .selectOption("home-logical-agent");
      } else {
        await expect(enabled).not.toBeChecked();
        await enabled.check();
      }
      await dialog
        .locator("select[name=embedding]")
        .selectOption("home-embedding@1.0.0");
      await dialog
        .locator("input[name=decider]")
        .fill("approved-decider@2.0.0");
      await dialog
        .getByRole("button", {
          name: ja ? "担当を割り当て" : "Assign agent",
          exact: true,
        })
        .click();
      if (native) {
        await expect(dialog.getByRole("alert")).toBeVisible();
        if (failure === "transport") {
          await expect(dialog.locator("select[name=agent]")).toBeDisabled();
          await expect(dialog.locator("select[name=embedding]")).toBeDisabled();
          await dialog
            .getByRole("button", {
              name: ja
                ? "同じ実行許可を再試行"
                : "Retry the same execution grant",
              exact: true,
            })
            .click();
          expect(grants[1]).toEqual(grants[0]);
        } else {
          await expect(dialog.locator("select[name=agent]")).toBeEnabled();
          await expect(dialog.locator("select[name=embedding]")).toBeEnabled();
          await dialog.locator("input[name=decider]").fill("replacement@2.0.0");
          await dialog
            .getByRole("button", {
              name: ja ? "担当を割り当て" : "Assign agent",
              exact: true,
            })
            .click();
          expect(grants[1].id).not.toEqual(grants[0].id);
        }
      }
      await expect(dialog).toHaveCount(0);
      expect(grants).toHaveLength(native ? 2 : 1);
      expect(grants.at(-1)).toMatchObject({
        node_id: "aidash://remote",
        agent: { id: "researcher", version: "1.0.0" },
        semantic: {
          mode: "required_home",
          embedding: { id: "home-embedding", version: "1.0.0" },
          decider: {
            id:
              failure === "create" || failure === "activate"
                ? "replacement"
                : "approved-decider",
            version: "2.0.0",
          },
          ...(native
            ? {
                native: {
                  participant: "home-logical-agent",
                  expected_revision: 7,
                  provider: { id: "native-memory", version: "1.0.0" },
                },
              }
            : {}),
        },
      });
      expect(errors).toEqual([]);
    });
  }

  test(`remote assignment respects disabled cross-conversation memory in ${locale}`, async ({
    page,
  }) => {
    const ja = locale === "ja-JP";
    const { errors } = await setup(page, {
      subject: true,
      locale,
      openTask: true,
      remoteAssignment: true,
      nativeMemory: true,
      nativeMemoryDisabled: true,
    });
    await page.goto("/collaboration?channel=workspace-one");
    await page
      .getByRole("button", {
        name: ja ? "タスクと成果物" : "Tasks and results",
        exact: true,
      })
      .click();
    await page
      .getByRole("dialog")
      .getByRole("button")
      .filter({ hasText: "Collect evidence" })
      .click();
    await page
      .getByRole("button", {
        name: ja ? "担当を割り当て" : "Assign agent",
        exact: true,
      })
      .click();
    const dialog = page.getByRole("dialog");
    await dialog
      .locator("select[name=agent]")
      .selectOption(JSON.stringify(["aidash://remote", "researcher", "1.0.0"]));
    const memory = dialog.getByLabel(
      ja
        ? "各推論の前に Home の記憶を検索する"
        : "Require Home memory before each inference",
    );
    await expect(memory).toBeEnabled();
    await expect(memory).not.toBeChecked();
    await memory.check();
    await dialog
      .getByLabel(
        ja
          ? "Homeの私有・共有記憶を参照する"
          : "Use private and shared memory at Home",
      )
      .check();
    const participant = dialog.getByLabel(
      ja ? "Homeの論理Agent" : "Home logical Agent",
      { exact: true },
    );
    await expect(
      participant.locator("option[value=home-logical-agent]"),
    ).toHaveCount(0);
    expect(errors).toEqual([]);
  });

  test(`hidden run retains only stop controls in ${locale}`, async ({
    page,
  }) => {
    const ja = locale === "ja-JP";
    const { errors } = await setup(page, { subject: true, locale });
    let control = "PAUSED";
    let semanticReason = "invalidated";
    const actions: unknown[] = [];
    await page.route("**/api/runs/run-0**", async (route) => {
      if (new URL(route.request().url()).pathname.endsWith("/management")) {
        if (route.request().method() === "POST") {
          actions.push(route.request().postDataJSON());
          control = "CANCELLED";
        }
        return route.fulfill({
          json: {
            id: "run-0",
            phase: "THINKING",
            control,
            may_control: true,
            semantic_reason: semanticReason,
            semantic_recovery: {
              cause: semanticReason,
              actions: ["create_follow_up_task"],
            },
            memory_cleanup: {
              state: "failed",
              invalidated: true,
              expires_at: "2026-10-06T00:00:00Z",
              attempts: 2,
              max_attempts: 2,
              next_attempt: "2026-10-06T00:00:00Z",
              last_error: "receiver_storage_unavailable",
            },
          },
        });
      }
      return route.fulfill({
        status: 403,
        json: {
          error: { message: "Current reader cannot access dependent content" },
        },
      });
    });
    await page.goto("/collaboration?channel=workspace-one");
    await page
      .getByRole("button", {
        name: ja ? "実行履歴" : "Execution history",
        exact: true,
      })
      .click();
    await page
      .getByRole("dialog")
      .getByRole("button")
      .filter({ hasText: "Researcher" })
      .click();
    const panel = page.getByRole("region", {
      name: ja ? "実行の管理" : "Execution management",
    });
    await expect(panel).toContainText(
      ja ? "参照済みのソース" : "A consumed source changed",
    );
    await expect(panel).toContainText(
      ja ? "受信コピーの清掃: 失敗" : "Receiver copy cleanup: Failed",
    );
    await expect(
      page
        .getByRole("dialog")
        .getByRole("button", { name: ja ? "再開" : "Resume", exact: true }),
    ).toHaveCount(0);
    semanticReason = "durable_memory_budget";
    await expect(panel).toContainText(ja ? "累積予算" : "cumulative budget");
    await expect(panel).not.toContainText(
      ja
        ? "元の Home Node で権限と参照元を確認して再開"
        : "Resume from the original Home node",
    );
    await panel
      .getByRole("button", {
        name: ja ? "実行を中止" : "Cancel execution",
        exact: true,
      })
      .click();
    await expect(panel.getByRole("button")).toHaveCount(0);
    expect(actions).toEqual([{ action: "cancel" }]);
    expect(errors).toEqual([]);
  });

  test(`a Run hidden by a withdrawn approval says why in ${locale}`, async ({
    page,
  }) => {
    const ja = locale === "ja-JP";
    const { errors } = await setup(page, { subject: true, locale });
    await page.route("**/api/runs/run-0**", (route) => {
      if (new URL(route.request().url()).pathname.endsWith("/management"))
        return route.fulfill({
          json: {
            id: "run-0",
            phase: "THINKING",
            control: "PAUSED",
            control_source: "deployment_administrator",
            may_control: true,
          },
        });
      return route.fulfill({
        status: 403,
        json: {
          error: "forbidden",
          run_reads: {
            kind: "definition_not_readable",
          },
        },
      });
    });
    await page.goto("/collaboration?channel=workspace-one");
    await page
      .getByRole("button", {
        name: ja ? "実行履歴" : "Execution history",
        exact: true,
      })
      .click();
    await page
      .getByRole("dialog")
      .getByRole("button")
      .filter({ hasText: "Researcher" })
      .click();
    await expect(page.getByRole("dialog")).toContainText(
      ja
        ? "この Run が読んだ定義を読む権限がありません。"
        : "Your permissions do not allow a definition this Run read.",
    );
    const panel = page.getByRole("region", {
      name: ja ? "実行の管理" : "Execution management",
    });
    await expect(panel).toContainText("Deployment Administrator");
    await expect(panel).not.toContainText(
      ja
        ? "元の Home Node で権限と参照元を確認して再開"
        : "Resume from the original Home node",
    );
    await expect(
      panel.getByRole("button", {
        name: ja ? "実行を中止" : "Cancel execution",
        exact: true,
      }),
    ).toBeVisible();
    expect(errors).toEqual([]);
  });

  for (const failure of ["transport", "create", "activate"]) {
    test(`remote generation preserves approval and activation bindings after ${failure} in ${locale}`, async ({
      page,
    }) => {
      const ja = locale === "ja-JP";
      const { errors } = await setup(page, {
        subject: true,
        locale,
        openTask: true,
        nativeMemory: true,
      });
      const intents: Record<string, unknown>[] = [];
      const grants: Record<string, unknown>[] = [];
      let approved = false;
      let unavailable = false;
      let activationAttempts = 0;
      let cancellations = 0;
      await page.route("**/api/generation/acme/policies", (route) =>
        route.fulfill({ json: [] }),
      );
      await page.route("**/api/tasks/task-0/remote-**", async (route) => {
        const path = new URL(route.request().url()).pathname;
        if (path.endsWith("/remote-generation")) {
          const body = route.request().postDataJSON();
          intents.push(body);
          if (unavailable)
            return route.fulfill({
              status: 403,
              json: { error: { message: "Current authority unavailable" } },
            });
          return route.fulfill({
            json: {
              intent_id: body.id,
              node_id: body.node_id,
              request_id: "receiver-request",
              agent: { id: "prepared-agent", version: "1.0.0" },
              prepared: approved,
              status: approved ? "QUEUED" : "PENDING_APPROVAL",
              expires_at: new Date(Date.now() + 3600000).toISOString(),
            },
          });
        }
        if (path.endsWith("/cancel")) {
          cancellations++;
          return route.fulfill({ json: true });
        }
        if (path.endsWith("/remote-grants")) {
          grants.push(route.request().postDataJSON());
          if (failure === "create" && grants.length === 1)
            return route.fulfill({
              status: 409,
              json: { error: { message: "Participant changed" } },
            });
          return route.fulfill({ json: { id: grants.at(-1)!.id } });
        }
        if (path.endsWith("/activate")) {
          activationAttempts++;
          if (activationAttempts === 1) {
            if (failure === "transport") return route.abort("failed");
            if (failure === "activate")
              return route.fulfill({
                status: 422,
                json: { error: { message: "Grant rejected" } },
              });
          }
          return route.fulfill({
            json: {
              run_id: "receiver-run",
              admission_id: "receiver-run",
              phase: "RECEIVED",
              control: "ACTIVE",
            },
          });
        }
        return route.fallback();
      });
      await page.goto("/collaboration?channel=workspace-one");
      await page
        .getByRole("button", {
          name: ja ? "タスクと成果物" : "Tasks and results",
          exact: true,
        })
        .click();
      await page
        .getByRole("dialog")
        .getByRole("button")
        .filter({ hasText: "Collect evidence" })
        .click();
      await page
        .getByRole("button", {
          name: ja ? "ポリシーで割り当て" : "Assign with policy",
          exact: true,
        })
        .click();
      const dialog = page.getByRole("dialog");
      await dialog
        .getByText(
          ja
            ? "別の Node で Agent を生成"
            : "Generate an agent at another node",
          { exact: true },
        )
        .click();
      const form = dialog
        .locator("details")
        .filter({ has: page.locator("input[name=node]") });
      await form
        .getByLabel(ja ? "実行 Node" : "Execution node", { exact: true })
        .fill("aidash://remote");
      await form
        .getByLabel(
          ja ? "実行 Node の生成ポリシー" : "Execution node generation policy",
          { exact: true },
        )
        .fill("specialist");
      await form
        .getByLabel(ja ? "生成を依頼する理由" : "Reason for generation", {
          exact: true,
        })
        .fill("Approved remote research");
      await form
        .getByRole("button", {
          name: ja ? "Agent を準備" : "Prepare agent",
          exact: true,
        })
        .click();
      await expect(
        form.getByRole("button", {
          name: ja ? "許可して実行" : "Authorize and execute",
          exact: true,
        }),
      ).toHaveCount(0);
      await form
        .getByRole("button", {
          name: ja ? "準備を中止" : "Cancel preparation",
          exact: true,
        })
        .click();
      await expect.poll(() => cancellations).toBe(1);
      await form
        .getByRole("button", {
          name: ja ? "Agent を準備" : "Prepare agent",
          exact: true,
        })
        .click();
      await expect.poll(() => intents.length).toBe(2);
      expect(intents[0].id).not.toBe(intents[1].id);
      approved = true;
      const refresh = form.getByRole("button", {
        name: ja
          ? "同じ準備・承認状態を再確認"
          : "Recheck the same preparation and approval",
        exact: true,
      });
      await refresh.click();
      await expect(
        form.getByText("prepared-agent@1.0.0", { exact: true }),
      ).toBeVisible();
      expect(intents[2]).toEqual(intents[1]);
      unavailable = true;
      await refresh.click();
      await expect(
        form.getByText("prepared-agent@1.0.0", { exact: true }),
      ).toHaveCount(0);
      unavailable = false;
      await refresh.click();
      await form
        .getByLabel(
          ja ? "Home の embedding 定義" : "Home embedding definition",
          {
            exact: true,
          },
        )
        .fill("home-embedding@1.0.0");
      await form
        .getByLabel(
          ja
            ? "Homeの私有・共有記憶を参照する"
            : "Use private and shared memory at Home",
        )
        .check();
      await form
        .getByLabel(
          ja
            ? "Homeの記憶設定テンプレート"
            : "Home memory configuration template",
          { exact: true },
        )
        .selectOption("home-logical-agent");
      await expect(form).toContainText(
        ja ? "私有記憶を空で開始" : "empty private memory",
      );
      const activate = form.getByRole("button", {
        name: ja ? "許可して実行" : "Authorize and execute",
        exact: true,
      });
      await activate.click();
      await expect(dialog.getByRole("alert")).toBeVisible();
      if (failure === "transport") {
        await expect(form.locator("input[name=lifetime]")).toBeDisabled();
        await form
          .getByRole("button", {
            name: ja
              ? "同じ実行許可を再試行"
              : "Retry the same execution grant",
            exact: true,
          })
          .click();
      } else {
        await expect(form.locator("input[name=lifetime]")).toBeEnabled();
        await form.locator("input[name=lifetime]").fill("600");
        await activate.click();
        expect(grants[1].id).not.toEqual(grants[0].id);
        expect(grants[1].ttl_seconds).toBe(600);
      }
      await expect(dialog).toHaveCount(0);
      expect(activationAttempts).toBe(failure === "create" ? 1 : 2);
      expect(grants).toHaveLength(2);
      if (failure === "transport") expect(grants[1]).toEqual(grants[0]);
      expect(grants[0]).toMatchObject({
        node_id: "aidash://remote",
        agent: { id: "prepared-agent", version: "1.0.0" },
        semantic: {
          mode: "required_home",
          embedding: { id: "home-embedding", version: "1.0.0" },
          native: {
            participant: "home-logical-agent",
            expected_revision: 7,
            provider: { id: "native-memory", version: "1.0.0" },
          },
        },
      });
      expect(errors).toEqual([]);
    });
  }

  test(`remote memory shows provenance and invalidation controls in ${locale}`, async ({
    page,
  }) => {
    const ja = locale === "ja-JP";
    const { errors } = await setup(page, { subject: true, locale });
    const grant = {
      id: "4191ce62-a1a8-42d6-9926-bd32f5e0dc34",
      task_id: "task-0",
      node_id: "aidash://remote",
      agent: { id: "researcher", version: "1.0.0" },
      expires_at: new Date(Date.now() + 3600000).toISOString(),
      revoked: false,
    };
    const execution = {
      grant_id: grant.id,
      admission_id: "829cb342-d2c8-43e4-99c2-0dbe96bc4f70",
      run_id: "829cb342-d2c8-43e4-99c2-0dbe96bc4f70",
      phase: "THINKING",
      control: "ACTIVE",
      error: null,
    };
    const semantic = {
      state: "ready",
      reason: null as string | null,
      operation_id: "c8cf392a-f1d4-42e1-8261-a003d1422660",
      retry_count: 0,
      retry_at: null,
      result_count: 2,
      truncated: false,
      retrieved_at: new Date().toISOString(),
      recovery: null as { cause: string; actions: string[] } | null,
    };
    let denied = false;
    let followup: Record<string, unknown> | null = null;
    await page.route("**/api/tasks/task-0/remote-**", async (route) => {
      const path = new URL(route.request().url()).pathname;
      if (path.endsWith("/remote-executions"))
        return route.fulfill({
          json: [{ grant, execution, semantic, unavailable: false }],
        });
      if (path.endsWith("/semantic"))
        return denied
          ? route.fulfill({
              status: 403,
              json: { error: { message: "forbidden" } },
            })
          : route.fulfill({
              json: {
                home_node: "aidash://home",
                operation_id: semantic.operation_id,
                executor: "aidash://remote/agents/researcher@1.0.0",
                binding: { mode: "required_home" },
                model: "home-vector",
                model_version: "1",
                retrieved_at: semantic.retrieved_at,
                truncated: false,
                allowance_node: "aidash://home",
                allowances: [
                  {
                    request_id: "origin-allowance",
                    used_tokens: 1234,
                    token_limit: 800000,
                    embedding_calls: 2,
                    embedding_call_limit: 10,
                    compaction_calls: 1,
                    compaction_call_limit: 4,
                  },
                ],
                sources: [
                  {
                    entry_id: "verified-source",
                    revision: 1,
                    content_digest: "sha256:fixture",
                    agent: null,
                  },
                ],
                memory: [
                  {
                    bank: {
                      home: "aidash://home",
                      tenant: "acme",
                      workspace: "workspace-one",
                      participant: "home-logical-agent",
                    },
                    provider: { id: "native-memory", version: "1.0.0" },
                    state: "ready",
                    units: [
                      {
                        id: "canonical-memory-unit",
                        revision: 7,
                        kind: "world",
                        verification: "unverified",
                        evidence: [
                          {
                            kind: "artifact",
                            id: "supporting-artifact",
                            revision: 2,
                            digest: "sha256:source",
                          },
                        ],
                        content_digest: "sha256:unit",
                      },
                    ],
                  },
                ],
              },
            });
      if (path.endsWith("/follow-up")) {
        followup = route.request().postDataJSON();
        return route.fulfill({ json: { id: "fresh-task" } });
      }
      if (path.endsWith("/control")) {
        execution.control = "CANCELLED";
        return route.fulfill({ json: execution });
      }
      return route.fallback();
    });
    await page.goto("/collaboration?channel=workspace-one");
    await page
      .getByRole("button", {
        name: ja ? "タスクと成果物" : "Tasks and results",
        exact: true,
      })
      .click();
    await page
      .getByRole("dialog")
      .getByRole("button")
      .filter({ hasText: "Collect evidence" })
      .click();
    const panel = page.getByRole("region", {
      name: ja ? "遠隔実行" : "Remote execution",
      exact: true,
    });
    await panel
      .getByRole("button", {
        name: ja ? "参照情報を確認" : "Inspect provenance",
        exact: true,
      })
      .click();
    await expect(
      panel.getByText("verified-source", { exact: true }),
    ).toBeVisible();
    await expect(
      panel.getByText("origin-allowance", { exact: true }),
    ).toBeVisible();
    await expect(panel).toContainText("1,234 / 800,000");
    await expect(
      panel.getByText("canonical-memory-unit", { exact: true }),
    ).toBeVisible();
    await expect(panel).toContainText(ja ? "未検証" : "Unverified");
    denied = true;
    await expect(
      panel.getByText("verified-source", { exact: true }),
    ).toHaveCount(0, { timeout: 12000 });
    await expect(
      panel.getByText("canonical-memory-unit", { exact: true }),
    ).toHaveCount(0);
    await expect(panel.getByRole("status")).toContainText(
      ja ? "現在の権限" : "current authority",
    );
    // A spent budget offers only a follow-up, with its specific explanation.
    execution.control = "PAUSED";
    for (const cause of ["context_budget", "durable_memory_budget"]) {
      semantic.state = "paused";
      semantic.reason = cause;
      semantic.recovery = {
        cause,
        actions: ["create_follow_up_task"],
      };
      await panel
        .getByRole("button", {
          name: ja ? "状態を再読み込み" : "Refresh status",
          exact: true,
        })
        .click();
      await expect(
        panel.getByRole("button", {
          name: ja ? "Follow-up Task を作成" : "Create follow-up task",
          exact: true,
        }),
      ).toBeVisible();
      await expect(
        panel.getByRole("button", {
          name: ja ? "権限を再確認して再開" : "Recheck authority and resume",
          exact: true,
        }),
      ).toHaveCount(0);
      await expect(panel.getByRole("status")).toContainText(
        cause === "durable_memory_budget"
          ? ja
            ? "累積予算"
            : "cumulative budget"
          : ja
            ? "コンテキスト上限"
            : "context budget",
      );
    }
    // An unscheduled outage authorizes the unchanged retry and a follow-up.
    semantic.reason = "unavailable";
    semantic.recovery = {
      cause: "unavailable",
      actions: ["retry_unchanged_binding", "create_follow_up_task"],
    };
    await panel
      .getByRole("button", {
        name: ja ? "状態を再読み込み" : "Refresh status",
        exact: true,
      })
      .click();
    await expect(
      panel.getByRole("button", {
        name: ja ? "権限を再確認して再開" : "Recheck authority and resume",
        exact: true,
      }),
    ).toBeVisible();
    semantic.state = "invalidated";
    semantic.reason = "invalidated";
    semantic.recovery = {
      cause: "invalidated",
      actions: ["create_follow_up_task"],
    };
    await panel
      .getByRole("button", {
        name: ja ? "状態を再読み込み" : "Refresh status",
        exact: true,
      })
      .click();
    await expect(
      panel.getByRole("button", {
        name: ja ? "権限を再確認して再開" : "Recheck authority and resume",
        exact: true,
      }),
    ).toHaveCount(0);
    await panel
      .getByRole("button", {
        name: ja ? "Follow-up Task を作成" : "Create follow-up task",
        exact: true,
      })
      .click();
    await panel
      .getByLabel(ja ? "タイトル" : "Title", { exact: true })
      .fill("New independent intent");
    await panel
      .getByLabel(ja ? "新しい指示" : "New instructions", { exact: true })
      .fill("Use currently authorized material.");
    await panel
      .getByRole("button", { name: ja ? "作成" : "Create", exact: true })
      .click();
    await expect
      .poll(() => followup)
      .toMatchObject({
        title: "New independent intent",
        description: "Use currently authorized material.",
        requirements: {},
      });
    expect(followup).not.toHaveProperty("parent_id");
    await panel
      .getByRole("button", {
        name: ja ? "実行を中止" : "Cancel execution",
        exact: true,
      })
      .click();
    await expect(
      panel.getByRole("button", {
        name: ja ? "実行を中止" : "Cancel execution",
        exact: true,
      }),
    ).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}

for (const viewport of [
  { width: 1440, height: 700 },
  { width: 390, height: 600 },
  { width: 900, height: 400 },
]) {
  test(`long registration dialog stays contained at ${viewport.width}x${viewport.height}`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    const { errors } = await setup(page, { subject: true });
    await page.goto("/settings?view=registry");
    await page
      .getByRole("button", { name: "Register entity", exact: true })
      .click();
    const dialog = page.getByRole("dialog");
    await dialog.getByLabel("Entity type").selectOption("model");
    const heading = dialog.getByRole("heading", { name: "Register entity" });
    // Measure after the open transition so only scrolling could move the heading.
    await dialog.evaluate((element) =>
      Promise.all(
        element
          .getAnimations({ subtree: true })
          .map((animation) => animation.finished),
      ),
    );
    const headingBefore = await heading.boundingBox();
    const submit = dialog.getByRole("button", {
      name: "Register entity",
      exact: true,
    });
    await submit.scrollIntoViewIfNeeded();
    await expect(submit).toBeInViewport();
    await expect(heading).toBeInViewport();
    expect(await heading.boundingBox()).toEqual(headingBefore);
    const bounds = await dialog.boundingBox();
    const buttonBounds = await submit.boundingBox();
    expect(bounds!.y).toBeGreaterThanOrEqual(0);
    expect(bounds!.y + bounds!.height).toBeLessThanOrEqual(viewport.height);
    expect(buttonBounds!.y + buttonBounds!.height).toBeLessThan(
      bounds!.y + bounds!.height,
    );
    expect(
      await dialog.evaluate(
        (element) => element.scrollWidth <= element.clientWidth,
      ),
    ).toBe(true);
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}

test("conversation is the landing view, with contextual progress and account settings", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "# Research", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "New request", exact: true }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: "View in graph", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Prepare a channel", exact: true }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("new request preserves the Japanese conversation creation contract", async ({
  page,
}) => {
  const { errors } = await setup(page, { subject: true, locale: "ja-JP" });
  let payload: unknown;
  await page.route("**/api/scratch-spaces**", (route) =>
    route.fulfill({ json: [{ id: "scratch-one", name: "共有領域" }] }),
  );
  await page.route("**/api/conversations", (route) => {
    payload = route.request().postDataJSON();
    return route.fulfill({ json: { workspace: { id: "workspace-two" } } });
  });
  await page.goto("/");
  await page.getByRole("button", { name: "新しい依頼", exact: true }).click();
  const dialog = page.getByRole("dialog");
  await dialog.getByLabel("タイトル", { exact: true }).fill("競合調査");
  await dialog
    .getByLabel("ゴール", { exact: true })
    .fill("Rustフレームワークを比較して");
  await dialog
    .getByRole("combobox", { name: "コーディネーター" })
    .selectOption("researcher@1.0.0");
  await dialog
    .getByRole("combobox", { name: "スクラッチスペース" })
    .selectOption("scratch-one");
  await dialog
    .getByRole("button", { name: "新しいゴール", exact: true })
    .click();
  await expect(dialog).toHaveCount(0);
  await expect(page).toHaveURL(/channel=workspace-two/);
  expect(payload).toEqual({
    node: defaultNode.name,
    title: "競合調査",
    goal: "Rustフレームワークを比較して",
    target: { id: "researcher", version: "1.0.0" },
    target_kind: "agent",
    scratch_space_id: "scratch-one",
  });
  expect(errors).toEqual([]);
});

test("failed message keeps its draft and idempotency key on retry", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, { failFirstMessage: true });
  await page.goto("/collaboration?channel=workspace-one");
  const composer = page.getByRole("textbox", { name: "Message", exact: true });
  await composer.fill("Please check the source.");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "Your text has been kept",
  );
  await expect(composer).toHaveValue("Please check the source.");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(composer).toHaveValue("");
  await expect(
    page.getByText("Please check the source.", { exact: true }),
  ).toBeVisible();
  const messages = submissions.filter((item) =>
    item.path.endsWith("/thread-messages"),
  );
  expect(messages).toHaveLength(2);
  expect(messages[0].body.idempotency_key).toBe(
    messages[1].body.idempotency_key,
  );
  expect(errors).toEqual([]);
});

test("message attachments show their names and download in the selected tab context", async ({
  page,
}) => {
  const { errors } = await setup(page, { messageAttachment: true });
  await page.goto("/collaboration?channel=workspace-one");
  const attachment = page.getByRole("button", {
    name: "Download attachment: evidence.txt",
  });
  await expect(attachment).toBeVisible();
  const [download, request] = await Promise.all([
    page.waitForEvent("download"),
    page.waitForRequest(
      (request) =>
        request.url().endsWith("/attachments/attachment-one") &&
        request.method() === "GET",
    ),
    attachment.click(),
  ]);
  expect(download.suggestedFilename()).toBe("evidence.txt");
  expect(request.headers()["x-aidash-context"]).toBe(
    "mapping:acme:fixture-mapping",
  );
  expect(request.headers()["authorization"]).toBeUndefined();
  expect(errors).toEqual([]);
});

test("preparing a channel does not invoke agent execution", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, { subject: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Settings", exact: true }).click();
  await page
    .getByRole("button", { name: "Prepare a channel", exact: true })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog
    .getByRole("textbox", { name: "Title", exact: true })
    .fill("Draft research");
  await dialog
    .getByRole("textbox", { name: "Goal", exact: true })
    .fill("Discuss sources before starting");
  await dialog.getByRole("button", { name: "Create", exact: true }).click();
  await expect(page).toHaveURL(/channel=prepared/);
  await expect(
    page.getByRole("heading", { name: "# Draft research", exact: true }),
  ).toBeVisible();
  expect(submissions.map((item) => item.path)).toEqual(["/api/workspaces"]);
  expect(errors).toEqual([]);
});

test("graph renders with Cytoscape and offers current related channel first", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.goto("/collaboration?channel=workspace-two");
  await page
    .getByRole("button", { name: "View in graph", exact: true })
    .click();
  await expect(page).toHaveURL(/\/graph\?channel=workspace-two/);
  await expect(page.locator(".collab-cytoscape canvas").first()).toBeVisible();
  const related = page.locator(".collab-selection .collab-channel-link");
  await expect(related).toHaveCount(2);
  await expect(related.first()).toHaveText("# Planning");
  await related.filter({ hasText: "Research" }).click();
  await expect(page).toHaveURL(/\/collaboration\?channel=workspace-one/);
  await expect(
    page.getByText("Evidence is ready to review.", { exact: true }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("graph focus survives contextual selection, reload, and history", async ({
  page,
}) => {
  const { errors } = await setup(page, {
    extraGraphAgent: true,
    subject: true,
  });
  const node = `aidash://home/${defaultNode.name}`;
  await page.goto("/collaboration?channel=workspace-one");
  await page.getByRole("button", { name: "Tasks and results" }).click();
  await page
    .getByRole("dialog")
    .getByRole("list", { name: "Agents working here" })
    .getByRole("button")
    .filter({ hasText: "Researcher" })
    .click();
  const selector = page.getByLabel("Focus agent");
  const original = JSON.stringify([
    "entity",
    node,
    "agent",
    "researcher",
    "1.0.0",
  ]);
  const focused = JSON.stringify([
    "entity",
    node,
    "agent",
    'review/"[special]:/agent',
    "1.0.0",
  ]);
  await expect(selector).toHaveValue(original);
  expect(new URL(page.url()).searchParams.get("focus")).toBe(original);

  await selector.selectOption(focused);
  await expect(selector).toHaveValue(focused);
  await expect(page.locator(".collab-cytoscape canvas").first()).toBeVisible();
  expect(new URL(page.url()).searchParams.get("focus")).toBe(focused);

  await page.reload();
  await expect(selector).toHaveValue(focused);
  await page.goBack();
  await expect(selector).toHaveValue(original);
  await page.goForward();
  await expect(selector).toHaveValue(focused);

  const unavailable = JSON.stringify([
    "entity",
    node,
    "agent",
    "unavailable",
    "1.0.0",
  ]);
  await page.goto(`/graph?focus=${encodeURIComponent(unavailable)}`);
  await expect(selector).toHaveValue("");
  await expect(
    page.getByRole("status").filter({
      hasText: "This resource is unavailable under your current access.",
    }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("channel intervention answers a human request without settings navigation", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, { subject: true });
  await page.goto("/");
  await page
    .getByRole("button")
    .filter({ hasText: "Which market should I examine?" })
    .click();
  const dialog = page.getByRole("dialog");
  await dialog
    .getByRole("textbox", { name: "Answer", exact: true })
    .fill("Japan");
  await dialog.getByRole("button", { name: "Send", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(
    submissions.some(
      (item) => item.path === "/api/human-requests/question-one/answer",
    ),
  ).toBe(true);
  expect(errors).toEqual([]);
});

for (const action of ["accept", "decline", "cancel"] as const) {
  test(`MCP elicitation ${action} sends a typed response from the Thread`, async ({
    page,
  }) => {
    const { submissions, errors } = await setup(page, {
      subject: true,
      elicitation: {
        message: "Choose the report settings",
        schema: {
          type: "object",
          properties: {
            market: {
              type: "string",
              title: "Market",
              enum: ["Japan", "Canada"],
            },
            count: { type: "integer", title: "Count", minimum: 1, maximum: 5 },
            publish: { type: "boolean", title: "Publish" },
          },
          required: ["market", "count", "publish"],
        },
      },
    });
    await page.goto("/");
    await page
      .getByRole("button")
      .filter({ hasText: "Choose the report settings" })
      .click();
    const dialog = page.getByRole("dialog");
    await expect(dialog).toContainText("Do not enter secrets or credentials");
    if (action === "accept") {
      await dialog
        .getByLabel("Market", { exact: true })
        .selectOption({ label: "Japan" });
      await dialog.getByLabel("Count", { exact: true }).fill("3");
      await dialog.getByLabel("Publish", { exact: true }).selectOption("false");
    }
    await dialog
      .getByRole("button", {
        name:
          action === "accept"
            ? "Accept"
            : action === "decline"
              ? "Decline"
              : "Cancel",
        exact: true,
      })
      .click();
    await expect(dialog).toHaveCount(0);
    expect(
      submissions.find(
        (item) => item.path === "/api/human-requests/question-one/answer",
      )?.body,
    ).toEqual(
      action === "accept"
        ? { action, content: { market: "Japan", count: 3, publish: false } }
        : { action },
    );
    expect(errors).toEqual([]);
  });
}

test("MCP elicitation remains usable when Content Security Policy blocks code evaluation", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, {
    subject: true,
    elicitation: {
      message: "Choose the report count",
      schema: {
        type: "object",
        properties: { count: { type: "integer", title: "Count", minimum: 1 } },
        required: ["count"],
      },
    },
  });
  await page.route("**/", async (route) => {
    const response = await route.fetch();
    await route.fulfill({
      response,
      headers: {
        ...response.headers(),
        "content-security-policy": "script-src 'self'; object-src 'none'",
      },
    });
  });
  await page.goto("/");
  await page
    .getByRole("button")
    .filter({ hasText: "Choose the report count" })
    .click();
  const dialog = page.getByRole("dialog");
  await expect(
    dialog.getByRole("button", { name: "Accept", exact: true }),
  ).toBeEnabled();
  await dialog.getByLabel("Count", { exact: true }).fill("2");
  await dialog.getByRole("button", { name: "Accept", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(
    submissions.find(
      (item) => item.path === "/api/human-requests/question-one/answer",
    )?.body,
  ).toEqual({ action: "accept", content: { count: 2 } });
  expect(errors).toEqual([]);
});

for (const [constraint, property] of [
  ["format", { type: "string", format: "server-specific-format" }],
  ["pattern", { type: "string", pattern: "(?i)hello" }],
] as const) {
  test(`MCP elicitation refuses an unavailable ${constraint} and still allows declining`, async ({
    page,
  }) => {
    const { submissions, errors } = await setup(page, {
      subject: true,
      elicitation: {
        message: "Choose a server-specific value",
        schema: { type: "object", properties: { value: property } },
      },
    });
    await page.goto("/");
    await page
      .getByRole("button")
      .filter({ hasText: "Choose a server-specific value" })
      .click();
    const dialog = page.getByRole("dialog");
    await expect(dialog.getByRole("alert")).toContainText(
      "This browser cannot validate this form's constraints",
    );
    await expect(
      dialog.getByRole("button", { name: "Accept", exact: true }),
    ).toBeDisabled();
    await dialog.getByRole("button", { name: "Decline", exact: true }).click();
    await expect(dialog).toHaveCount(0);
    expect(
      submissions.find(
        (item) => item.path === "/api/human-requests/question-one/answer",
      )?.body,
    ).toEqual({ action: "decline" });
    expect(errors).toEqual([]);
  });
}

test("MCP elicitation shows constants and defaults and preserves their primitive types", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, {
    subject: true,
    elicitation: {
      message: "Confirm the fixed settings",
      schema: {
        type: "object",
        properties: {
          market: { type: "string", title: "Market", const: "Japan" },
          empty: { type: "string", title: "Empty", const: "" },
          zero: { type: "integer", title: "Zero", const: 0 },
          publish: { type: "boolean", title: "Publish", const: false },
          label: { type: "string", title: "Label", default: "Quarterly" },
          count: { type: "number", title: "Count", default: 0 },
          optional: { type: "boolean", title: "Optional", default: false },
          choice: {
            type: "string",
            title: "Choice",
            enum: ["", "Japan"],
            default: "",
          },
        },
        required: ["market", "empty", "zero", "publish"],
      },
    },
  });
  await page.goto("/");
  await page
    .getByRole("button")
    .filter({ hasText: "Confirm the fixed settings" })
    .click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByLabel("Market", { exact: true })).toHaveValue(
    "Japan",
  );
  await expect(dialog.getByLabel("Zero", { exact: true })).toHaveValue("0");
  await expect(dialog.getByLabel("Publish", { exact: true })).toHaveValue(
    "false",
  );
  await expect(dialog.getByLabel("Label", { exact: true })).toHaveValue(
    "Quarterly",
  );
  await dialog.getByRole("button", { name: "Accept", exact: true }).click();
  await expect(dialog).toHaveCount(0);
  expect(
    submissions.find(
      (item) => item.path === "/api/human-requests/question-one/answer",
    )?.body,
  ).toEqual({
    action: "accept",
    content: {
      market: "Japan",
      empty: "",
      zero: 0,
      publish: false,
      label: "Quarterly",
      count: 0,
      optional: false,
      choice: "",
    },
  });
  expect(errors).toEqual([]);
});

for (const { constraint, property, invalid, valid } of [
  {
    constraint: "pattern",
    property: { type: "string", pattern: "[A-Z]{2}" },
    invalid: "lowercase",
    valid: "prefixJPtail",
  },
  {
    constraint: "multipleOf",
    property: { type: "number", minimum: 0.1, multipleOf: 0.5 },
    invalid: "0.6",
    valid: "0.5",
  },
  {
    constraint: "fractional multipleOf",
    property: { type: "number", multipleOf: 0.1 },
    invalid: "0.31",
    valid: "0.3",
  },
  {
    constraint: "precise multipleOf",
    property: { type: "number", multipleOf: 1 },
    invalid: "1.00000001",
    valid: "2",
  },
  {
    constraint: "exclusiveMinimum",
    property: { type: "integer", exclusiveMinimum: 2 },
    invalid: "2",
    valid: "3",
  },
  {
    constraint: "exclusiveMaximum",
    property: { type: "number", exclusiveMaximum: 4 },
    invalid: "4",
    valid: "3.5",
  },
  {
    constraint: "enum constraints",
    property: { type: "number", enum: [1, 2], minimum: 2 },
    invalid: "1",
    valid: "2",
  },
  {
    constraint: "format",
    property: { type: "string", format: "email" },
    invalid: "invalid-address",
    valid: "person@example.com",
  },
  {
    constraint: "Unicode minLength",
    property: { type: "string", minLength: 2 },
    invalid: "😀",
    valid: "😀a",
  },
  {
    constraint: "Unicode maxLength",
    property: { type: "string", maxLength: 2 },
    invalid: "abc",
    valid: "😀a",
  },
]) {
  test(`MCP elicitation validates ${constraint} before sending an answer`, async ({
    page,
  }) => {
    const { submissions, errors } = await setup(page, {
      subject: true,
      elicitation: {
        message: "Choose a constrained value",
        schema: {
          type: "object",
          properties: { value: { ...property, title: "Value" } },
          required: ["value"],
        },
      },
    });
    await page.goto("/");
    await page
      .getByRole("button")
      .filter({ hasText: "Choose a constrained value" })
      .click();
    const dialog = page.getByRole("dialog");
    const control = dialog.getByLabel("Value", { exact: true });
    const enter = (value: string) =>
      "enum" in property
        ? control.selectOption({ label: value })
        : control.fill(value);
    await enter(invalid);
    await dialog.getByRole("button", { name: "Accept", exact: true }).click();
    await expect(dialog).toBeVisible();
    await expect(dialog.getByRole("alert")).toBeVisible();
    expect(
      submissions.filter(
        (item) => item.path === "/api/human-requests/question-one/answer",
      ),
    ).toEqual([]);
    await enter(valid);
    await dialog.getByRole("button", { name: "Accept", exact: true }).click();
    await expect(dialog).toHaveCount(0);
    expect(
      submissions.find(
        (item) => item.path === "/api/human-requests/question-one/answer",
      )?.body,
    ).toEqual({
      action: "accept",
      content: { value: property.type === "string" ? valid : Number(valid) },
    });
    expect(errors).toEqual([]);
  });
}

test("a waiting question says when its Run continues on its own", async ({
  page,
}) => {
  const { errors } = await setup(page, {
    subject: true,
    answerBy: "2026-10-21T10:30:00Z",
  });
  await page.goto("/");
  const card = page
    .getByRole("button")
    .filter({ hasText: "Which market should I examine?" });
  await expect(card).toContainText(
    /The Run continues on its own at Oct \d+, \d{2}:\d{2}/,
  );
  await card.click();
  await expect(page.getByRole("dialog")).toContainText(
    /The Run continues on its own at Oct \d+, \d{2}:\d{2}/,
  );
  expect(errors).toEqual([]);
});

test("a question without an Answer Deadline names no time to continue", async ({
  page,
}) => {
  await setup(page, { subject: true });
  await page.goto("/");
  const card = page
    .getByRole("button")
    .filter({ hasText: "Which market should I examine?" });
  await expect(card).toBeVisible();
  await expect(card).not.toContainText("continues on its own");
});

test("a waiting question names its time to continue in Japanese", async ({
  page,
}) => {
  await setup(page, {
    subject: true,
    locale: "ja-JP",
    answerBy: "2026-10-21T10:30:00Z",
  });
  await page.goto("/");
  await expect(
    page
      .getByRole("button")
      .filter({ hasText: "Which market should I examine?" }),
  ).toContainText("に Run が自身の判断で続行します");
});

test("a question closed at the Answer Deadline reads as unanswered", async ({
  page,
}) => {
  const { errors, passAnswerDeadline } = await setup(page, {
    subject: true,
    answerBy: "2026-10-21T10:30:00Z",
  });
  await page.goto("/");
  await page
    .getByRole("button")
    .filter({ hasText: "Which market should I examine?" })
    .click();
  const dialog = page.getByRole("dialog");
  await expect(
    dialog.getByRole("textbox", { name: "Answer", exact: true }),
  ).toBeVisible();

  passAnswerDeadline();
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await expect(dialog.getByRole("status")).toHaveText(
    "No answer before the Answer Deadline",
    { timeout: 15_000 },
  );
  await expect(dialog).toContainText("Which market should I examine?");
  await expect(dialog.getByRole("textbox")).toHaveCount(0);
  await expect(dialog).not.toContainText("continues on its own");

  await dialog.getByRole("button", { name: "Close", exact: true }).click();
  await page.goto("/collaboration?channel=workspace-one");
  await expect(
    page.getByText(
      "No answer before the Answer Deadline: Which market should I examine?",
    ),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("revoked history is hidden rather than retaining a stale conversation", async ({
  page,
}) => {
  const { revokeHistory } = await setup(page);
  await page.goto("/collaboration?channel=workspace-one");
  await expect(
    page.getByText("Evidence is ready to review.", { exact: true }),
  ).toBeVisible();
  revokeHistory();
  await expect(
    page.getByText("Evidence is ready to review.", { exact: true }),
  ).toHaveCount(0);
  await expect(page.getByRole("alert")).toContainText("unavailable");
});

test("an inaccessible explicit channel is not silently replaced with another channel", async ({
  page,
}) => {
  await setup(page);
  await page.goto("/collaboration?channel=unavailable");
  await expect(
    page.getByRole("heading", { name: "# Research", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("heading", {
      name: "This resource is unavailable under your current access.",
      exact: true,
    }),
  ).toBeVisible();
});

test("legacy management links are routed into secondary settings", async ({
  page,
}) => {
  const { errors } = await setup(page, { subject: true });
  await page.goto("/agents");
  await expect(page).toHaveURL(/\/settings\?.*view=agents/);
  await expect(
    page.getByRole("heading", { name: "Researcher", exact: true }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("mobile conversation and channel switching do not require horizontal scrolling", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/collaboration?channel=workspace-one");
  await expect(
    page.getByRole("textbox", { name: "Message", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Channels", exact: true }).click();
  await page.getByRole("link", { name: "# Planning", exact: true }).click();
  await expect(
    page.getByRole("heading", { name: "# Planning", exact: true }),
  ).toBeVisible();
  expect(
    await page.evaluate(
      () => document.documentElement.scrollWidth <= window.innerWidth,
    ),
  ).toBe(true);
  expect(errors).toEqual([]);
});

test("thread replies persist separately and channel drafts survive thread navigation", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page);
  await page.goto("/collaboration?channel=workspace-one");
  const composer = page.getByRole("textbox", { name: "Message", exact: true });
  await composer.fill("Unsent channel note");
  await page
    .getByRole("button", { name: "Reply in thread", exact: true })
    .click();
  await expect(
    page.getByRole("heading", { name: "Thread", exact: true }),
  ).toBeVisible();
  await expect(composer).toHaveValue("Unsent channel note");
  const reply = page.getByRole("textbox", { name: "Reply", exact: true });
  await expect(reply).toHaveValue("");
  await reply.fill("Here is the supporting source.");
  await page.getByRole("button", { name: "Send reply", exact: true }).click();
  await expect(
    page.getByText("Here is the supporting source.", { exact: true }),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Back to channel", exact: true })
    .click();
  await expect(composer).toHaveValue("Unsent channel note");
  await expect(
    page.getByText("Here is the supporting source.", { exact: true }),
  ).toHaveCount(0);
  await page.getByRole("button", { name: "Open thread", exact: true }).click();
  await expect(
    page.getByText("Here is the supporting source.", { exact: true }),
  ).toBeVisible();
  const posts = submissions.filter((item) =>
    item.path.endsWith("/thread-messages"),
  );
  expect(posts).toHaveLength(1);
  expect(posts[0].body.thread_id).toBe("thread-message-one");
  expect(errors).toEqual([]);
});

test("older message pages are prepended without losing the latest messages", async ({
  page,
}) => {
  const { errors } = await setup(page, { olderMessages: 45 });
  await page.goto("/collaboration?channel=workspace-one");
  const feed = page.getByRole("region", { name: "Conversation", exact: true });
  await expect(feed.getByRole("article")).toHaveCount(40);
  await expect(
    page.getByText("Previous message 0", { exact: true }),
  ).toHaveCount(0);
  await page
    .getByRole("button", { name: "Load older messages", exact: true })
    .click();
  await expect(feed.getByRole("article")).toHaveCount(46);
  await expect(
    page.getByText("Previous message 0", { exact: true }),
  ).toBeVisible();
  await expect(
    page.getByText("Evidence is ready to review.", { exact: true }),
  ).toHaveCount(1);
  await expect(
    page.getByRole("button", { name: "Load older messages", exact: true }),
  ).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("local run details retain execution memory", async ({ page }) => {
  const { errors } = await setup(page, { subject: true, runMemory: true });
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Execution history", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByRole("button")
    .filter({ hasText: "Researcher" })
    .click();
  await page.getByText("Memory", { exact: true }).click();
  await expect(
    page.getByRole("dialog").locator("details").filter({ hasText: "Memory" }),
  ).toContainText("Retained execution memory.");
  expect(errors).toEqual([]);
});

test("run message attachments stay fixed while an upload is pending", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, { subject: true });
  let uploadStarted!: () => void;
  let finishUpload!: () => void;
  const started = new Promise<void>((resolve) => (uploadStarted = resolve));
  const uploadGate = new Promise<void>((resolve) => (finishUpload = resolve));
  await page.route(
    "**/api/workspaces/workspace-one/attachments?*",
    async (route) => {
      uploadStarted();
      await uploadGate;
      await route.fallback();
    },
  );
  await page.route("**/api/runs/run-0/message", (route) =>
    route.fulfill({ json: { id: "run-message-one" } }),
  );
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Execution history", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByRole("button")
    .filter({ hasText: "Researcher" })
    .click();
  const dialog = page.getByRole("dialog");
  const draft = dialog.getByRole("textbox", { name: "Message", exact: true });
  const picker = dialog.locator('input[type="file"]');
  await draft.fill("Inspect this image");
  await picker.setInputFiles({
    name: "sample.png",
    mimeType: "image/png",
    buffer: Buffer.from("\x89PNG\r\n\x1a\nfixture"),
  });
  const remove = dialog.getByRole("button", {
    name: "Remove attachment: sample.png",
  });
  await dialog.getByRole("button", { name: "Send", exact: true }).click();
  await started;
  await expect(draft).toBeDisabled();
  await expect(picker).toBeDisabled();
  await expect(remove).toBeDisabled();
  await expect(
    dialog.getByRole("button", { name: /^(Send|Uploading…)$/ }),
  ).toBeDisabled();
  finishUpload();
  await expect(dialog).toHaveCount(0);
  expect(
    submissions.filter((item) => item.path.endsWith("/attachments")),
  ).toHaveLength(1);
  expect(errors).toEqual([]);
});

test("run media picker follows the effective model route before uploads", async ({
  page,
}) => {
  const { submissions, errors, setRunMediaRoutes } = await setup(page, {
    subject: true,
    runMediaRoutes: [],
  });
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Execution history", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByRole("button")
    .filter({ hasText: "Researcher" })
    .click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.locator('input[type="file"]')).toHaveCount(0);
  await expect(
    dialog.getByRole("textbox", { name: "Message", exact: true }),
  ).toBeVisible();
  expect(
    submissions.filter((item) => item.path.endsWith("/attachments")),
  ).toHaveLength(0);
  setRunMediaRoutes([["image/png"]]);
  await expect(dialog.locator('input[type="file"]')).toBeVisible();
  const picker = dialog.locator('input[type="file"]');
  await picker.setInputFiles({
    name: "sample.png",
    mimeType: "image/png",
    buffer: Buffer.from("\x89PNG\r\n\x1a\nfixture"),
  });
  await expect(
    dialog.getByRole("button", { name: "Remove attachment: sample.png" }),
  ).toBeVisible();
  setRunMediaRoutes([]);
  await expect(picker).toHaveCount(0);
  await dialog.getByRole("button", { name: "Send", exact: true }).click();
  await expect(dialog.getByRole("alert")).toBeVisible();
  expect(
    submissions.filter((item) => item.path.endsWith("/attachments")),
  ).toHaveLength(0);
  expect(errors).toEqual([]);
});

test("mobile history remains reachable from graph and settings", async ({
  page,
}) => {
  await setup(page);
  await page.setViewportSize({ width: 390, height: 844 });
  for (const url of ["/graph", "/settings?view=agents"]) {
    await page.goto(url);
    await page.getByRole("button", { name: "Channels", exact: true }).click();
    await expect(
      page.getByRole("link", { name: "# Research", exact: true }),
    ).toBeVisible();
    await page
      .getByRole("button", { name: "Close history", exact: true })
      .click();
    await expect(
      page.getByRole("dialog", { name: "Menu", exact: true }),
    ).toHaveCount(0);
  }
});

test("a same-length refreshed page follows a new latest message", async ({
  page,
}) => {
  const { errors } = await setup(page, {
    latestMessageChangesOnPoll: true,
    olderMessages: 39,
  });
  await page.goto("/collaboration?channel=workspace-one");
  const feed = page.getByRole("region", { name: "Conversation", exact: true });
  await expect(feed.getByRole("article")).toHaveCount(40);
  await expect(
    page.getByText("New latest message.", { exact: true }),
  ).toBeVisible({
    timeout: 5000,
  });
  await expect(feed.getByRole("article")).toHaveCount(40);
  await expect
    .poll(async () =>
      feed.evaluate((element) => {
        const messages = element.querySelectorAll("article");
        const latest = messages[messages.length - 1];
        return (
          Math.abs(
            latest.getBoundingClientRect().bottom -
              element.getBoundingClientRect().bottom,
          ) < 5
        );
      }),
    )
    .toBe(true);
  expect(errors).toEqual([]);
});

test("message following keeps context after the latest message and respects reading older history", async ({
  page,
}) => {
  const { errors } = await setup(page, { olderMessages: 39 });
  // On narrow screens pending requests follow the latest message inside the feed.
  await page.setViewportSize({ width: 390, height: 844 });
  await page.goto("/collaboration?channel=workspace-one");
  const feed = page.getByRole("region", { name: "Conversation", exact: true });
  await expect(feed.getByRole("article")).toHaveCount(40);
  await expect
    .poll(() =>
      feed.evaluate((element) => {
        const latest = element.querySelectorAll("article")[39];
        const bottom = element.getBoundingClientRect().bottom;
        return (
          Math.abs(latest.getBoundingClientRect().bottom - bottom) < 5 &&
          element.scrollHeight - element.scrollTop - element.clientHeight > 80
        );
      }),
    )
    .toBe(true);
  const history: ChannelMessagePage = await page.evaluate(() =>
    fetch("/api/workspaces/workspace-one/message-history").then((response) =>
      response.json(),
    ),
  );
  await page.route(
    "**/api/workspaces/workspace-one/message-history**",
    (route) => route.fulfill({ json: history }),
  );
  await feed.evaluate((element) => {
    element.scrollTop = 0;
    element.dispatchEvent(new Event("scroll"));
  });
  const latest = history.messages.at(-1)!;
  latest.message = {
    ...latest.message,
    id: "manual-reading-update",
    content: "Update while reading older history.",
  };
  await expect(feed.locator("#message-manual-reading-update")).toBeAttached();
  expect(await feed.evaluate((element) => element.scrollTop)).toBeLessThan(10);
  await page
    .getByRole("button", { name: "Jump to latest messages", exact: true })
    .click();
  latest.message = {
    ...latest.message,
    id: "following-update",
    content: "Update after resuming following.",
  };
  await expect(feed.locator("#message-following-update")).toBeAttached();
  await expect
    .poll(() =>
      feed.evaluate((element) => {
        const latest = element.querySelector("#message-following-update")!;
        return (
          Math.abs(
            latest.getBoundingClientRect().bottom -
              element.getBoundingClientRect().bottom,
          ) < 5
        );
      }),
    )
    .toBe(true);
  await expect(
    page.getByRole("button", { name: "Jump to latest messages", exact: true }),
  ).toHaveCount(0);
  expect(errors).toEqual([]);
});

for (const locale of ["en-US", "ja-JP"] as const) {
  test(`progress overview remains reachable after every detail tab in ${locale}`, async ({
    page,
  }) => {
    const ja = locale === "ja-JP";
    if (ja) await page.setViewportSize({ width: 390, height: 1000 });
    const { errors } = await setup(page, { subject: true, locale });
    await page.goto("/collaboration?channel=workspace-one");
    const open = page.getByRole("button", {
      name: ja ? "状況を表示" : "Show channel status",
      exact: true,
    });
    await open.click();
    const sheet = page.getByRole("dialog", {
      name: ja ? "依頼の進捗" : "Request progress",
    });
    const nav = sheet.getByRole("tablist");
    const pause = sheet.getByRole("button", {
      name: ja
        ? "このチャンネルの実行を一時停止"
        : "Pause runs in this channel",
      exact: true,
    });
    await expect(pause).toBeVisible();
    for (const name of ja
      ? ["タスクと成果物", "成果物", "実行履歴"]
      : ["Tasks and results", "Artifacts", "Execution history"]) {
      await nav.getByRole("tab", { name, exact: true }).click();
      await expect(pause).toHaveCount(0);
      await nav
        .getByRole("tab", { name: ja ? "概要" : "Overview", exact: true })
        .click();
      await expect(pause).toBeVisible();
    }
    await nav
      .getByRole("tab", {
        name: ja ? "タスクと成果物" : "Tasks and results",
        exact: true,
      })
      .click();
    await sheet
      .getByRole("button", { name: ja ? "閉じる" : "Close", exact: true })
      .click();
    await expect(sheet).toHaveCount(0);
    await open.click();
    await expect(pause).toBeVisible();
    expect(errors).toEqual([]);
  });
}

for (const action of ["task", "participant", "new task"] as const) {
  test(`overview ${action} replaces the progress sheet with one dismissible dialog`, async ({
    page,
  }) => {
    const { errors } = await setup(page, { subject: true });
    await page.goto("/collaboration?channel=workspace-one");
    const open = page.getByRole("button", {
      name: "Show channel status",
      exact: true,
    });
    await open.click();
    const sheet = page.getByRole("dialog", { name: "Request progress" });
    if (action === "task")
      await sheet
        .getByRole("button")
        .filter({ hasText: "Collect evidence" })
        .click();
    else if (action === "participant")
      await sheet.getByRole("button").filter({ hasText: "Researcher" }).click();
    else
      await sheet
        .getByRole("button", { name: "Assign a task", exact: true })
        .click();
    await expect(sheet).toHaveCount(0);
    const dialog = page.getByRole("dialog");
    await expect(dialog).toHaveCount(1);
    if (action === "task")
      await expect(
        dialog.getByRole("heading", { name: "Collect evidence", exact: true }),
      ).toBeVisible();
    else if (action === "participant")
      await expect(dialog).toContainText("Thinking");
    else {
      await expect(dialog.getByLabel("Title", { exact: true })).toHaveValue("");
      await expect(dialog.getByLabel("Workspace", { exact: true })).toHaveValue(
        "workspace-one",
      );
    }
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    await open.click();
    await expect(sheet).toBeVisible();
    await page.keyboard.press("Escape");
    await expect(page.getByRole("dialog")).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}

test("revoking a thread read hides cached replies without exposing another conversation", async ({
  page,
}) => {
  const { revokeThreads } = await setup(page);
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Reply in thread", exact: true })
    .click();
  const threadPanel = page.getByRole("complementary", {
    name: "Thread",
    exact: true,
  });
  await expect(
    threadPanel.getByText("Evidence is ready to review.", { exact: true }),
  ).toBeVisible();
  revokeThreads();
  await expect(page.getByRole("alert")).toContainText("unavailable");
  await expect(threadPanel.getByRole("article")).toHaveCount(0);
  await expect(
    threadPanel.getByRole("textbox", { name: "Reply", exact: true }),
  ).toHaveCount(0);
  await expect(
    page.getByRole("textbox", { name: "Message", exact: true }),
  ).toBeVisible();
});

test("attachment retries preserve uploads, files and message identity", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, { failFirstMessage: true });
  await page.goto("/");
  await page
    .getByRole("textbox", { name: "Message", exact: true })
    .fill("Please review the source.");
  await page.locator('input[type="file"]').setInputFiles({
    name: "source.txt",
    mimeType: "text/plain",
    buffer: Buffer.from("evidence"),
  });
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText(
    "Your text has been kept",
  );
  await expect(
    page.getByRole("button", { name: "Remove attachment: source.txt" }),
  ).toBeVisible();
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Download attachment: source.txt" }),
  ).toBeVisible();
  await expect(
    page.getByRole("button", { name: /^Remove attachment:/ }),
  ).toHaveCount(0);
  const uploads = submissions.filter((item) =>
    item.path.endsWith("/attachments"),
  );
  const messages = submissions.filter((item) =>
    item.path.endsWith("/thread-messages"),
  );
  expect(uploads).toHaveLength(1);
  expect(uploads[0].body.size).toBe(8);
  expect(messages).toHaveLength(2);
  expect(messages[0].body).toEqual(messages[1].body);
  expect(messages[0].body.attachment_ids).toHaveLength(1);
  expect(errors).toEqual([]);
});

test("upload failures retain their key and file limits apply before network use", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, { failFirstUpload: true });
  await page.goto("/");
  const files = page.locator('input[type="file"]');
  await files.setInputFiles({
    name: "large.txt",
    mimeType: "text/plain",
    buffer: Buffer.alloc(8 * 1024 * 1024 + 1),
  });
  await expect(page.getByRole("alert")).toContainText("8 MiB");
  expect(submissions).toHaveLength(0);
  await files.setInputFiles({
    name: "source.txt",
    mimeType: "text/plain",
    buffer: Buffer.from("evidence"),
  });
  await page
    .getByRole("textbox", { name: "Message", exact: true })
    .fill("Source");
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(page.getByRole("alert")).toContainText("upload unavailable");
  expect(
    submissions.filter((item) => item.path.endsWith("/thread-messages")),
  ).toHaveLength(0);
  await page.getByRole("button", { name: "Send", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Download attachment: source.txt" }),
  ).toBeVisible();
  const uploads = submissions.filter((item) =>
    item.path.endsWith("/attachments"),
  );
  expect(uploads).toHaveLength(2);
  expect(uploads[0].body.idempotency_key).toBe(uploads[1].body.idempotency_key);
  expect(errors).toEqual([]);
});

test("channel controls scope run changes and persist approval responses", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, {
    subject: true,
    approval: true,
  });
  await page.goto("/");
  await page
    .getByRole("button", { name: "Show channel status", exact: true })
    .click();
  await page
    .getByRole("button", { name: "Pause runs in this channel", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Resume runs in this channel" }),
  ).toBeEnabled();
  expect(
    submissions
      .filter((item) => item.path.endsWith("/control"))
      .map((item) => item.path),
  ).toEqual(["/api/runs/run-0/control"]);
  await page
    .getByRole("button", { name: "Resume runs in this channel" })
    .click();
  await expect(
    page.getByRole("button", { name: "Pause runs in this channel" }),
  ).toBeEnabled();
  await page.getByRole("button", { name: "Close", exact: true }).click();
  await page.getByRole("button", { name: "Approve", exact: true }).click();
  await expect(
    page.getByRole("button", { name: "Approve", exact: true }),
  ).toHaveCount(0);
  expect(
    submissions.find((item) => item.path.endsWith("/answer"))?.body,
  ).toEqual({ approved: true });
  expect(errors).toEqual([]);
});

test("channel can resume a paused run outside the state window", async ({
  page,
}) => {
  await setup(page, { subject: true });
  const data = fixture();
  await page.route("**/api/state**", (route) =>
    route.fulfill({
      json: {
        ...data,
        runs: [],
        access: { kind: "subject", tenant: "acme", subject: "alice" },
        next_cursors: { ...data.next_cursors, runs: "older" },
      },
    }),
  );
  await page.route("**/api/workspaces/workspace-one/runs?**", (route) =>
    route.fulfill({
      json: {
        items: [{ ...data.runs[0], control: "PAUSED" }],
        next_cursor: null,
      },
    }),
  );
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Show channel status", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Resume runs in this channel" }),
  ).toBeEnabled();
});

test("search, notifications, thread navigation and persisted theme work", async ({
  page,
}) => {
  const { errors } = await setup(page);
  await page.goto("/");
  await expect(
    page.getByRole("heading", { name: "# Research", exact: true }),
  ).toBeVisible();
  await page.keyboard.press("ControlOrMeta+K");
  await page
    .getByRole("searchbox", { name: "Search history" })
    .fill("next release");
  await expect(page.getByRole("link", { name: "# Research" })).toHaveCount(0);
  await expect(page.getByRole("link", { name: "# Planning" })).toBeVisible();
  await page.getByRole("searchbox", { name: "Search history" }).fill("");
  await page.keyboard.press("Escape");
  await page
    .getByRole("button", { name: "Notifications", exact: true })
    .click();
  await page
    .locator(".intent-notifications")
    .getByRole("button", { name: /Which market should I examine\?/ })
    .click();
  await expect(page.locator(".intent-notifications")).toHaveCount(0);
  await expect(
    page.getByRole("dialog", { name: "Human input", exact: true }),
  ).toBeVisible();
  await page
    .getByRole("dialog")
    .getByRole("button", { name: "Close", exact: true })
    .click();
  await expect(page.getByRole("dialog")).toHaveCount(0);
  await expect(page.locator("html")).toHaveAttribute("data-theme", "dark");
  await page
    .getByRole("button", { name: "Switch to light theme", exact: true })
    .click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await page.reload();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await page
    .getByRole("button", { name: "Account settings", exact: true })
    .click();
  await page.getByRole("radio", { name: "System", exact: true }).click();
  await expect(
    page.getByRole("radio", { name: "System", exact: true }),
  ).toBeChecked();
  expect(await page.evaluate(() => localStorage.getItem("aidash-theme"))).toBe(
    "system",
  );
  await page.keyboard.press("Escape");
  await page
    .getByRole("button", { name: "Request actions", exact: true })
    .click();
  await page
    .getByRole("menuitem", { name: "Threads in this channel", exact: true })
    .click();
  await expect(
    page.getByText("No threads in the loaded history."),
  ).toBeVisible();
  await page
    .getByRole("button", { name: "Show channel status", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByRole("tab", { name: "Tasks and results", exact: true })
    .click();
  await expect(
    page.getByRole("button", { name: "Add task", exact: true }),
  ).toBeVisible();
  expect(errors).toEqual([]);
});

test("composer preserves IME input and safely renders inline formatting", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page);
  await page.goto("/");
  const composer = page.getByRole("textbox", { name: "Message", exact: true });
  await composer.fill("draft");
  await composer.press("Shift+Enter");
  await expect(composer).toHaveValue("draft\n");
  await composer.dispatchEvent("keydown", {
    key: "Enter",
    code: "Enter",
    isComposing: true,
  });
  expect(submissions).toHaveLength(0);
  await composer.fill("**Important** <script>alert('xss')</script>");
  await composer.press("Enter");
  await expect(composer).toHaveValue("");
  const latest = page
    .getByRole("region", { name: "Conversation", exact: true })
    .getByRole("article")
    .last();
  await expect(latest.locator("strong")).toHaveText("Important");
  await expect(latest).toContainText("<script>");
  expect(errors).toEqual([]);
});

for (const viewport of [
  { width: 390, height: 844 },
  { width: 900, height: 400 },
]) {
  test(`composer and status remain in view at ${viewport.width}x${viewport.height}`, async ({
    page,
  }) => {
    await page.setViewportSize(viewport);
    const { errors } = await setup(page, { olderMessages: 45 });
    await page.goto("/");
    await expect(
      page.getByRole("textbox", { name: "Message", exact: true }),
    ).toBeInViewport();
    await expect(
      page.getByRole("button", { name: "Send", exact: true }),
    ).toBeInViewport();
    await page.getByRole("button", { name: "Show channel status" }).click();
    await expect(
      page.getByRole("button", { name: "Pause runs in this channel" }),
    ).toBeInViewport();
    await page.getByRole("button", { name: "Close", exact: true }).click();
    await expect(
      page.getByRole("textbox", { name: "Message", exact: true }),
    ).toBeInViewport();
    expect(
      await page.evaluate(
        () => document.documentElement.scrollWidth <= innerWidth,
      ),
    ).toBe(true);
    expect(errors).toEqual([]);
  });
}

test("workspace reference renders light, dark and thread layouts", async ({
  page,
}, testInfo) => {
  await page.setViewportSize({ width: 1280, height: 800 });
  const { errors } = await setup(page, {
    subject: true,
    referenceLayout: true,
    locale: "ja-JP",
  });
  await page.goto("/");
  await expect(
    page.getByRole("button", { name: "承認する", exact: true }),
  ).toBeVisible();
  await page.getByRole("button", { name: "状況を表示", exact: true }).click();
  await expect(
    page.getByRole("button", {
      name: "このチャンネルの実行を一時停止",
      exact: true,
    }),
  ).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("workspace-dark.png") });
  await page.getByRole("dialog").screenshot({
    path: testInfo.outputPath("workspace-status.png"),
  });
  await page.keyboard.press("Escape");
  await page
    .locator("form")
    .filter({ has: page.getByRole("textbox", { name: "メッセージ" }) })
    .screenshot({
      path: testInfo.outputPath("workspace-composer.png"),
    });
  await page
    .getByRole("button", { name: "ライトテーマに切り替え", exact: true })
    .click();
  await expect(page.locator("html")).toHaveAttribute("data-theme", "light");
  await page.screenshot({
    path: testInfo.outputPath("workspace-light.png"),
    animations: "disabled",
  });
  await page
    .locator("#message-message-one")
    .getByRole("button", { name: "スレッドで返信" })
    .click();
  await expect(
    page.getByRole("textbox", { name: "返信", exact: true }),
  ).toBeVisible();
  await page.screenshot({ path: testInfo.outputPath("workspace-thread.png") });
  await page.setViewportSize({ width: 1280, height: 960 });
  await page.screenshot({
    path: testInfo.outputPath("workspace-thread-tall.png"),
  });
  expect(errors).toEqual([]);
});

test("shared files can be inspected without leaving the workspace", async ({
  page,
}) => {
  const { errors } = await setup(page, { messageAttachment: true });
  await page.goto("/");
  await page.getByRole("button", { name: "Files", exact: true }).click();
  const files = page.getByRole("region", { name: "Shared channel files" });
  await expect(
    files.getByRole("button", { name: "Download attachment: evidence.txt" }),
  ).toBeVisible();
  await files.getByRole("button", { name: "Preview: evidence.txt" }).click();
  const dialog = page.getByRole("dialog");
  await expect(
    dialog.getByRole("heading", { name: "evidence.txt" }),
  ).toBeVisible();
  await expect(dialog.locator("pre")).toHaveText("Source evidence");
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(1);
  await page.keyboard.press("Escape");
  await expect(dialog).toHaveCount(0);
  expect(errors).toEqual([]);
});

for (const width of [1440, 390]) {
  test(`remote execution reconciles controls and retries one instruction at ${width}px`, async ({
    page,
  }) => {
    await page.setViewportSize({ width, height: 800 });
    const { errors } = await setup(page, { subject: true });
    const grant = {
      id: "4191ce62-a1a8-42d6-9926-bd32f5e0dc34",
      task_id: "task-0",
      node_id: "aidash://remote",
      agent: { id: "researcher", version: "1.0.0" },
      expires_at: new Date(Date.now() + 3600000).toISOString(),
      revoked: false,
    };
    const execution = {
      grant_id: grant.id,
      admission_id: "829cb342-d2c8-43e4-99c2-0dbe96bc4f70",
      run_id: "829cb342-d2c8-43e4-99c2-0dbe96bc4f70",
      phase: "THINKING",
      control: "ACTIVE",
      error: null,
    };
    const humanRequests = [
      {
        id: "human-question",
        kind: "QUESTION",
        prompt: "Which source should the remote Agent use?",
        response: null as unknown,
      },
      {
        id: "human-approval",
        kind: "APPROVAL_REQUIRED",
        prompt: "Approve the saved remote operation?",
        response: null as unknown,
      },
    ];
    const answers: { id: string; response: unknown }[] = [];
    let unavailable = false;
    const messages: { id: string; content: string }[] = [];
    await page.route("**/api/tasks/task-0/remote-**", async (route) => {
      const path = new URL(route.request().url()).pathname;
      if (path.endsWith("/remote-executions"))
        return route.fulfill({
          json: [
            {
              grant,
              execution: unavailable ? null : execution,
              unavailable,
              human_requests: humanRequests,
            },
          ],
        });
      if (path.endsWith("/human-requests/answer")) {
        const input = route.request().postDataJSON();
        answers.push(input);
        const request = humanRequests.find((item) => item.id === input.id)!;
        request.response = input.response;
        return answers.length === 1
          ? route.fulfill({
              status: 503,
              json: { error: { message: "Answer receipt unavailable" } },
            })
          : route.fulfill({ json: request });
      }
      if (path.endsWith("/control")) {
        const action = route.request().postDataJSON().action;
        execution.control = {
          pause: "PAUSED",
          resume: "ACTIVE",
          cancel: "CANCELLED",
        }[action as "pause"];
        return route.fulfill({ json: execution });
      }
      if (path.endsWith("/messages")) {
        messages.push(route.request().postDataJSON());
        return messages.length === 1
          ? route.fulfill({
              status: 503,
              json: {
                error: { message: "Receipt unavailable. Retry to reconcile." },
              },
            })
          : route.fulfill({
              json: {
                id: messages[0].id,
                run_id: execution.run_id,
                accepted: true,
              },
            });
      }
      return route.fallback();
    });
    await page.goto("/collaboration?channel=workspace-one");
    await page.getByRole("button", { name: "Tasks and results" }).click();
    await page
      .getByRole("dialog")
      .getByRole("button")
      .filter({ hasText: "Collect evidence" })
      .click();
    const panel = page.getByRole("region", { name: "Remote execution" });
    await expect(
      panel.getByText("aidash://remote", { exact: true }),
    ).toBeVisible();
    await expect(panel.getByText(humanRequests[0].prompt)).toBeVisible();
    await panel
      .getByLabel("Response", { exact: true })
      .fill("Use the original source.");
    await panel
      .getByRole("button", { name: "Send response", exact: true })
      .click();
    await expect(panel.getByRole("alert")).toContainText(
      "Answer receipt unavailable",
    );
    await panel
      .getByRole("button", { name: "Send response", exact: true })
      .click();
    await expect(panel.getByLabel("Response", { exact: true })).toHaveCount(0);
    expect(answers[1]).toEqual(answers[0]);
    await panel.getByRole("button", { name: "Approve", exact: true }).click();
    await expect(
      panel.getByRole("button", { name: "Approve", exact: true }),
    ).toHaveCount(0);
    expect(answers[2]).toEqual({
      id: "human-approval",
      response: { approved: true },
    });
    await panel.getByRole("button", { name: "Pause", exact: true }).click();
    const resume = panel.getByRole("button", {
      name: "Recheck authority and resume",
    });
    await expect(resume).toBeVisible();
    await resume.focus();
    await page.keyboard.press("Enter");
    await expect(
      panel.getByRole("button", { name: "Pause", exact: true }),
    ).toBeVisible();
    const input = panel.getByLabel("Instruction for the remote Agent");
    await input.fill("Keep the original source and report the uncertainty.");
    await panel.getByRole("button", { name: "Send instruction" }).click();
    await expect(input).toBeDisabled();
    await expect(panel.getByRole("alert")).toContainText("Receipt unavailable");
    await panel
      .getByRole("button", { name: "Retry the same instruction" })
      .click();
    await expect(panel.getByRole("status")).toContainText(
      "Instruction acceptance confirmed",
    );
    expect(messages).toHaveLength(2);
    expect(messages[1]).toEqual(messages[0]);
    await expect(input).toHaveValue("");
    unavailable = true;
    await panel.getByRole("button", { name: "Refresh status" }).click();
    await expect(
      panel.getByText("The destination is unavailable.", { exact: false }),
    ).toBeVisible();
    unavailable = false;
    grant.expires_at = "2000-01-01T00:00:00Z";
    execution.control = "PAUSED";
    await panel.getByRole("button", { name: "Refresh status" }).click();
    await expect(panel.getByText("Expired", { exact: false })).toBeVisible();
    await expect(
      panel.getByRole("button", { name: "Recheck authority and resume" }),
    ).toHaveCount(0);
    await expect(
      panel.getByRole("button", { name: "Cancel execution" }),
    ).toBeEnabled();
    const dialog = page.getByRole("dialog");
    expect(
      await dialog.evaluate(
        (element) => element.scrollWidth <= element.clientWidth,
      ),
    ).toBe(true);
    await panel.getByRole("button", { name: "Cancel execution" }).click();
    await expect(
      panel.getByRole("button", { name: "Cancel execution" }),
    ).toHaveCount(0);
    await page.keyboard.press("Escape");
    await expect(dialog).toHaveCount(0);
    expect(errors).toEqual([]);
  });
}

for (const pause of [
  "invalid_context",
  "invalidated",
  "context_budget",
  "durable_memory_budget",
]) {
  test(`paused ${pause} remains inspectable and cancellable with resume disabled`, async ({
    page,
  }) => {
    const { submissions, errors } = await setup(
      page,
      pause === "invalid_context"
        ? { subject: true, invalidRun: true }
        : { subject: true, semanticPause: pause },
    );
    await page.goto("/collaboration?channel=workspace-one");
    await page
      .getByRole("button", { name: "Show channel status", exact: true })
      .click();
    await expect(
      page.getByRole("button", {
        name: "Resume runs in this channel",
        exact: true,
      }),
    ).toBeDisabled();
    await page.keyboard.press("Escape");
    await page
      .getByRole("button", { name: "Execution history", exact: true })
      .click();
    await page
      .getByRole("dialog")
      .getByRole("button")
      .filter({ hasText: "Researcher" })
      .click();
    const dialog = page.getByRole("dialog");
    if (pause === "invalid_context") {
      await expect(dialog.getByRole("alert")).toContainText(
        "invalid execution context",
      );
    }
    await expect(
      dialog.getByRole("button", { name: "Resume", exact: true }),
    ).toBeDisabled();
    page.once("dialog", (dialog) => dialog.accept());
    await dialog.getByRole("button", { name: "Cancel", exact: true }).click();
    await expect
      .poll(
        () =>
          submissions.filter(
            (item) =>
              item.path.endsWith("/control") && item.body.action === "cancel",
          ).length,
      )
      .toBe(1);
    expect(errors).toEqual([]);
  });
}

test("paused failure delivery can resume while retaining its invalid Context diagnostic", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, {
    subject: true,
    failureDeliveryRun: true,
  });
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Execution history", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByRole("button")
    .filter({ hasText: "Researcher" })
    .click();
  const dialog = page.getByRole("dialog");
  await expect(dialog.getByRole("alert")).toContainText(
    "invalid execution context",
  );
  const resume = dialog.getByRole("button", { name: "Resume", exact: true });
  await expect(resume).toBeEnabled();
  await resume.click();
  await expect
    .poll(
      () =>
        submissions.filter(
          (item) =>
            item.path.endsWith("/control") && item.body.action === "resume",
        ).length,
    )
    .toBe(1);
  expect(errors).toEqual([]);
});

test("channel controls resume a validated failure delivery with an invalid Context", async ({
  page,
}) => {
  const { submissions, errors } = await setup(page, {
    subject: true,
    failureDeliveryRun: true,
  });
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Show channel status", exact: true })
    .click();
  const resume = page.getByRole("button", {
    name: "Resume runs in this channel",
    exact: true,
  });
  await expect(resume).toBeEnabled();
  await resume.click();
  await expect
    .poll(
      () =>
        submissions.filter(
          (item) =>
            item.path === "/api/runs/run-0/control" &&
            item.body.action === "resume",
        ).length,
    )
    .toBe(1);
  expect(errors).toEqual([]);
});

for (const collision of [false, true]) {
  test(`remote inspection supplies native requirements despite ${collision ? "a mismatched Home provider" : "an absent Home provider"} and resets them on selection`, async ({
    page,
  }) => {
    const { errors } = await setup(page, {
      subject: true,
      openTask: true,
      remoteAssignment: true,
      nativeMemory: true,
      remoteNativeCollision: collision,
      extraGraphAgent: true,
    });
    const grants: Record<string, unknown>[] = [];
    await page.route("**/api/tasks/task-0/remote-grants**", async (route) => {
      const path = new URL(route.request().url()).pathname;
      if (path.endsWith("/inspect")) {
        const selected = route.request().postDataJSON().agent.id;
        return route.fulfill({
          json: {
            native_required: selected === "researcher",
            memory_available: selected === "researcher",
          },
        });
      }
      if (path.endsWith("/remote-grants")) {
        grants.push(route.request().postDataJSON());
        return route.fulfill({ json: { id: grants.at(-1)!.id } });
      }
      return route.fulfill({
        json: {
          admission_id: "remote-run",
          phase: "RECEIVED",
          control: "ACTIVE",
        },
      });
    });
    await page.goto("/collaboration?channel=workspace-one");
    await page
      .getByRole("button", { name: "Tasks and results", exact: true })
      .click();
    await page
      .getByRole("dialog")
      .getByRole("button")
      .filter({ hasText: "Collect evidence" })
      .click();
    await page
      .getByRole("button", { name: "Assign agent", exact: true })
      .click();
    const dialog = page.getByRole("dialog");
    const agent = dialog.locator("select[name=agent]");
    await agent.selectOption(
      JSON.stringify(["aidash://remote", "researcher", "1.0.0"]),
    );
    const memory = dialog.getByLabel(
      "Require Home memory before each inference",
    );
    await expect(memory).toBeChecked();
    await expect(memory).toBeDisabled();
    const other = await agent
      .locator("option")
      .evaluateAll((options) =>
        options
          .map((option) => (option as HTMLOptionElement).value)
          .find((value) => value && !value.includes('"researcher"')),
      );
    expect(other).toBeTruthy();
    await agent.selectOption(other!);
    await expect(memory).not.toBeChecked();
    await expect(memory).toBeDisabled();
    await expect(dialog.locator("select[name=embedding]")).toHaveCount(0);
    await dialog
      .getByRole("button", { name: "Assign agent", exact: true })
      .click();
    await expect.poll(() => grants.length).toBe(1);
    expect(grants[0].semantic).toEqual({ mode: "disabled" });
    expect(errors).toEqual([]);
  });
}

test("topology renders each discovered Node once with its agents and cross-Node communication", async ({
  page,
}) => {
  const { errors } = await setup(page);
  const data = fixture();
  const local = data.deployment.id;
  const remote = "aidash://remote/remote-node";
  const entity = data.registry.find((entry) => entry.kind === "agent")!;
  data.tasks[0].created_by = `${local}/agents/${entity.id}@${entity.version}`;
  data.tasks[0].owner = `${remote}/agents/${entity.id}@${entity.version}`;
  await page.route("**/api/state**", (route) => route.fulfill({ json: data }));
  await page.route("**/api/discover**", (route) =>
    route.fulfill({
      json: {
        agents: [
          { node_id: local, entity },
          { node_id: remote, entity },
        ],
        failures: [],
      },
    }),
  );
  await page.goto("/graph?channel=workspace-one");
  await page.getByLabel("Graph perspective").selectOption("neighborhood");
  await page
    .getByRole("button", {
      name: "Node topology and communication",
      exact: true,
    })
    .click();
  const topology = page.getByRole("img", {
    name: "Mesh topology",
  });
  await expect(topology).toBeVisible();
  await expect(
    topology.locator("text").filter({ hasText: "1 Agents" }),
  ).toHaveCount(2);
  await expect(topology.locator("path[marker-end]")).toHaveCount(1);
  expect(errors).toEqual([]);
});

const boundSpaceId = "11111111-1111-4111-8111-111111111111";
const boundSpace = `/api/scratch-spaces/${boundSpaceId}`;
const namedSpace = {
  id: boundSpaceId,
  node: defaultNode.name,
  name: "Tabechao build",
  creator: "alice",
  members: [],
  state: "active",
  revision: 0,
  wipe_generation: 0,
  created_at: "2026-01-01T00:00:00Z",
  last_used_at: "2026-01-01T00:00:00Z",
  expired: false,
  settings: {
    capacity_bytes: 1073741824,
    idle_expiry_days: 14,
    allowed_hosts: { kind: "hosts", hosts: [] },
    execution_image: null,
    external_services: [],
    cpu_millis: 1000,
    memory_bytes: 1073741824,
    daily_sent_bytes: 1073741824,
    daily_received_bytes: 1073741824,
    docker_enabled: false,
  },
};

const namedLimits = {
  ceilings: {
    capacity_bytes: 10737418240,
    cpu_millis: 4000,
    memory_bytes: 10737418240,
    daily_sent_bytes: 10737418240,
    daily_received_bytes: 10737418240,
    docker_allowed: false,
    all_hosts_allowed: false,
  },
  tenant_hosts: { kind: "hosts", hosts: [] },
};

for (const locale of ["en-US", "ja-JP"] as const) {
  const ja = locale === "ja-JP";
  test(`conversation header names its Scratch Space and links to it in ${locale}`, async ({
    page,
  }) => {
    const { errors } = await setup(page, { subject: true, locale });
    await page.route("**/api/scratch-spaces**", (route) => {
      const path = new URL(route.request().url()).pathname;
      if (path === boundSpace) return route.fulfill({ json: namedSpace });
      if (path.endsWith("/limits")) return route.fulfill({ json: namedLimits });
      return route.fulfill({ json: [namedSpace] });
    });
    // Short enough that the chooser sits below the fold until the page
    // scrolls to it.
    await page.setViewportSize({ width: 1280, height: 500 });
    await page.goto("/collaboration?channel=workspace-one");
    const link = page.getByRole("link", {
      name: "Tabechao build",
      exact: true,
    });
    // The header renders only after the read answers, so the id is checked
    // once the name shows; before that the page is empty and proves nothing.
    await expect(link).toBeVisible();
    await expect(page.getByText(boundSpaceId)).toHaveCount(0);
    await link.click();
    await expect(page).toHaveURL(/\/settings\?/);
    await expect(page).toHaveURL(/view=node/);
    await expect(page).toHaveURL(/channel=workspace-one/);
    const chooser = page.getByLabel(
      ja ? "スクラッチスペースを選択" : "Choose a Scratch Space",
      { exact: true },
    );
    await expect(chooser).toHaveValue(boundSpaceId);
    await expect(chooser).toBeInViewport();
    expect(errors).toEqual([]);
  });

  test(`Scratch Space settings scroll to the focused space once its form has rendered in ${locale}`, async ({
    page,
  }) => {
    await setup(page, { subject: true, locale });
    // The list answers first and the limits later, as on a slow Node.
    let releaseLimits = () => {};
    const limitsHeld = new Promise<void>((resolve) => {
      releaseLimits = resolve;
    });
    await page.route("**/api/scratch-spaces**", async (route) => {
      const path = new URL(route.request().url()).pathname;
      if (path.endsWith("/limits")) {
        await limitsHeld;
        return route.fulfill({ json: namedLimits });
      }
      if (path === boundSpace) return route.fulfill({ json: namedSpace });
      return route.fulfill({ json: [namedSpace] });
    });
    await page.setViewportSize({ width: 1280, height: 500 });
    await page.goto(
      `/settings?view=node&channel=workspace-one&focus=${boundSpaceId}`,
    );
    await expect(
      page.getByLabel(
        ja ? "スクラッチスペースを選択" : "Choose a Scratch Space",
        { exact: true },
      ),
    ).toHaveValue(boundSpaceId);
    releaseLimits();
    const save = page.getByRole("button", {
      name: ja ? "設定を保存" : "Save settings",
      exact: true,
    });
    await expect(save).toBeVisible();
    const heading = page.getByRole("heading", {
      name: ja ? "スクラッチスペース" : "Scratch Space",
      exact: true,
    });
    // The section's heading sits at the top of the pane that scrolls. Scrolling
    // before the form rendered stops short, because the page was still short.
    await expect
      .poll(() =>
        heading.evaluate((element) => {
          let pane = element.parentElement;
          while (
            pane &&
            !(
              pane.scrollHeight > pane.clientHeight &&
              /auto|scroll/.test(getComputedStyle(pane).overflowY)
            )
          )
            pane = pane.parentElement;
          if (!pane) return Number.NaN;
          return (
            element.getBoundingClientRect().top -
            pane.getBoundingClientRect().top
          );
        }),
      )
      .toBeLessThanOrEqual(1);
  });

  test(`conversation header warns about an expired Scratch Space in ${locale}`, async ({
    page,
  }) => {
    await setup(page, { subject: true, locale });
    await page.route(`**${boundSpace}`, (route) =>
      route.fulfill({ json: { ...namedSpace, expired: true } }),
    );
    await page.goto("/collaboration?channel=workspace-one");
    await expect(
      page.getByRole("link", { name: "Tabechao build", exact: true }),
    ).toBeVisible();
    await expect(
      page.getByText(
        ja
          ? "期限切れ。次の実行時に内容が消去されます"
          : "Expired; contents will be cleared on the next Run",
      ),
    ).toBeVisible();
  });

  test(`conversation header follows its Scratch Space as the event stream reports changes in ${locale}`, async ({
    page,
  }) => {
    await setup(page, { subject: true, locale });
    // Each reconnect of the closed stream reports one event, which refreshes
    // the dashboard queries.
    await page.route("**/api/events/stream", (route) =>
      route.fulfill({
        status: 200,
        contentType: "text/event-stream",
        body: "id: 1\n\n",
      }),
    );
    let current: { status: number; body: unknown } = {
      status: 200,
      body: namedSpace,
    };
    await page.route(`**${boundSpace}`, (route) =>
      route.fulfill({ status: current.status, json: current.body }),
    );
    await page.goto("/collaboration?channel=workspace-one");
    await expect(
      page.getByRole("link", { name: "Tabechao build", exact: true }),
    ).toBeVisible();
    const expired = page.getByText(
      ja
        ? "期限切れ。次の実行時に内容が消去されます"
        : "Expired; contents will be cleared on the next Run",
    );
    await expect(expired).toHaveCount(0);
    // The space expires; the warning appears without a reload.
    current = { status: 200, body: { ...namedSpace, expired: true } };
    await expect(expired).toBeVisible();
    // Another member deletes it; the name gives way to the unavailable label.
    current = { status: 404, body: { error: "gone" } };
    await expect(
      page.getByText(
        ja ? "使えないスクラッチスペース" : "Unavailable Scratch Space",
        { exact: true },
      ),
    ).toBeVisible();
    await expect(
      page.getByRole("link", { name: "Tabechao build", exact: true }),
    ).toHaveCount(0);
    await expect(page.getByText(boundSpaceId)).toHaveCount(0);
  });

  for (const status of [403, 404]) {
    test(`conversation header hides a Scratch Space refused with ${status} in ${locale}`, async ({
      page,
    }) => {
      await setup(page, { subject: true, locale });
      await page.route(`**${boundSpace}`, (route) =>
        route.fulfill({ status, json: { error: "refused" } }),
      );
      await page.goto("/collaboration?channel=workspace-one");
      await expect(
        page.getByText(
          ja ? "使えないスクラッチスペース" : "Unavailable Scratch Space",
          { exact: true },
        ),
      ).toBeVisible();
      await expect(page.getByText(boundSpaceId)).toHaveCount(0);
    });
  }

  test(`conversation header reports a failed Scratch Space read in ${locale}`, async ({
    page,
  }) => {
    await setup(page, { subject: true, locale });
    await page.route(`**${boundSpace}`, (route) =>
      route.fulfill({ status: 500, json: { error: "unavailable" } }),
    );
    await page.goto("/collaboration?channel=workspace-one");
    await expect(
      page.getByText(
        ja
          ? "スクラッチスペースを読み込めませんでした"
          : "Could not load the Scratch Space",
        { exact: true },
      ),
    ).toBeVisible();
    await expect(
      page.getByText(
        ja ? "使えないスクラッチスペース" : "Unavailable Scratch Space",
      ),
    ).toHaveCount(0);
  });
}

for (const locale of ["en-US", "ja-JP"] as const) {
  test(`Scratch Space creation and destructive confirmations in ${locale}`, async ({
    page,
  }) => {
    const ja = locale === "ja-JP";
    await setup(page, { subject: true, locale });
    let created = false;
    let cleared = 0;
    let deleted = 0;
    const space = {
      id: "space-one",
      node: defaultNode.name,
      name: "Project",
      creator: "alice",
      members: [],
      state: "active",
      revision: 0,
      wipe_generation: 0,
      created_at: "2026-01-01T00:00:00Z",
      last_used_at: "2026-01-01T00:00:00Z",
      expired: true,
      settings: {
        capacity_bytes: 1073741824,
        idle_expiry_days: 14,
        allowed_hosts: { kind: "hosts", hosts: [] },
        execution_image: null,
        external_services: [],
        cpu_millis: 1000,
        memory_bytes: 1073741824,
        daily_sent_bytes: 1073741824,
        daily_received_bytes: 1073741824,
        docker_enabled: false,
      },
    };
    await page.route("**/api/scratch-spaces**", async (route) => {
      const url = new URL(route.request().url());
      if (url.pathname.endsWith("/limits"))
        return route.fulfill({
          json: {
            ceilings: {
              capacity_bytes: 10737418240,
              cpu_millis: 4000,
              memory_bytes: 10737418240,
              daily_sent_bytes: 10737418240,
              daily_received_bytes: 10737418240,
              docker_allowed: false,
              all_hosts_allowed: false,
            },
            tenant_hosts: { kind: "hosts", hosts: [] },
          },
        });
      if (url.pathname.endsWith("/clear")) {
        cleared++;
        return route.fulfill({ status: 204 });
      }
      if (route.request().method() === "DELETE") {
        deleted++;
        return route.fulfill({ status: 204 });
      }
      if (route.request().method() === "POST") {
        expect(route.request().postDataJSON()).toMatchObject({
          name: "Project",
          node: defaultNode.name,
        });
        created = true;
        return route.fulfill({ json: space });
      }
      return route.fulfill({ json: created ? [space] : [] });
    });
    await page.goto("/settings?view=node");
    await page
      .getByLabel(
        ja ? "新しいスクラッチスペースの名前" : "New Scratch Space name",
      )
      .fill("Project");
    await page
      .getByRole("button", {
        name: ja ? "スクラッチスペースを作成" : "Create Scratch Space",
        exact: true,
      })
      .click();
    await expect(
      page.getByText(
        ja
          ? "期限切れ。次の実行時に内容が消去されます"
          : "Expired; contents will be cleared on the next Run",
      ),
    ).toBeVisible();
    page.once("dialog", (dialog) => dialog.dismiss());
    await page
      .getByRole("button", {
        name: ja ? "スクラッチスペースを消去" : "Clear Scratch Space",
        exact: true,
      })
      .click();
    expect(cleared).toBe(0);
    page.once("dialog", (dialog) => dialog.accept());
    await page
      .getByRole("button", {
        name: ja ? "スクラッチスペースを消去" : "Clear Scratch Space",
        exact: true,
      })
      .click();
    await expect.poll(() => cleared).toBe(1);
    page.once("dialog", (dialog) => dialog.dismiss());
    await page
      .getByRole("button", {
        name: ja ? "スクラッチスペースを削除" : "Delete Scratch Space",
        exact: true,
      })
      .click();
    expect(deleted).toBe(0);
    page.once("dialog", (dialog) => dialog.accept());
    await page
      .getByRole("button", {
        name: ja ? "スクラッチスペースを削除" : "Delete Scratch Space",
        exact: true,
      })
      .click();
    await expect.poll(() => deleted).toBe(1);
  });
}

for (const failDecline of [false, true]) {
  test(`a paused call is declined from the Run details ${failDecline ? "and a refusal keeps the message and the pause" : "with the sentence sent first"}`, async ({
    page,
  }) => {
    const { submissions, errors } = await setup(page, {
      subject: true,
      declinableCall: true,
      failDecline,
    });
    await page.goto("/collaboration?channel=workspace-one");
    await page
      .getByRole("button", { name: "Execution history", exact: true })
      .click();
    await page
      .getByRole("dialog")
      .getByRole("button")
      .filter({ hasText: "Researcher" })
      .click();
    const dialog = page.getByRole("dialog");
    await dialog
      .getByRole("button", { name: "Decline this call", exact: true })
      .click();
    await expect(dialog.getByText("task_delegate")).toBeVisible();
    await dialog
      .getByLabel("Tell the Agent why (optional)")
      .fill("Use the shared notes instead.");
    await dialog
      .getByRole("button", { name: "Decline and continue", exact: true })
      .click();
    await expect
      .poll(() =>
        submissions
          .filter(
            (item) =>
              item.path === "/api/runs/run-0/message" ||
              item.path === "/api/runs/run-0/declined-calls",
          )
          .map((item) => item.path),
      )
      .toEqual(["/api/runs/run-0/message", "/api/runs/run-0/declined-calls"]);
    expect(
      submissions.find((item) => item.path === "/api/runs/run-0/declined-calls")
        ?.body,
    ).toEqual({ call: "run-0:3:0" });
    if (failDecline) {
      // The message stays sent, the Run stays paused and the form is still open to retry.
      await expect(
        dialog.getByRole("button", { name: "Decline and continue" }),
      ).toBeVisible();
      // A retry declines again and never sends the message twice.
      await dialog
        .getByRole("button", { name: "Decline and continue", exact: true })
        .click();
      await expect
        .poll(
          () =>
            submissions.filter(
              (item) => item.path === "/api/runs/run-0/declined-calls",
            ).length,
        )
        .toBe(2);
      expect(
        submissions.filter((item) => item.path === "/api/runs/run-0/message"),
      ).toHaveLength(1);
    } else {
      await expect(
        dialog.getByRole("button", { name: "Decline this call" }),
      ).toHaveCount(0);
    }
    expect(errors.filter((error) => !error.includes("409"))).toEqual([]);
  });
}

test("the decline button is absent when the server offers no call", async ({
  page,
}) => {
  const { errors } = await setup(page, {
    subject: true,
    semanticPause: "invalidated",
  });
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Execution history", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByRole("button")
    .filter({ hasText: "Researcher" })
    .click();
  await expect(
    page.getByRole("button", { name: "Decline this call" }),
  ).toHaveCount(0);
  expect(errors).toEqual([]);
});

test("a hidden Run offers declining a paused call without a sentence field", async ({
  page,
}) => {
  const { errors } = await setup(page, { subject: true });
  const declined: Record<string, unknown>[] = [];
  let control = "PAUSED";
  await page.route("**/api/runs/run-0**", (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/declined-calls")) {
      declined.push(route.request().postDataJSON());
      control = "ACTIVE";
      return route.fulfill({ json: { id: "run-0", control } });
    }
    if (path.endsWith("/management"))
      return route.fulfill({
        json: {
          id: "run-0",
          phase: "THINKING",
          control,
          control_source: "tenant",
          may_control: false,
          ...(control === "PAUSED"
            ? {
                declinable_call: {
                  call: "run-0:3:0",
                  tool: "task_delegate",
                  kind: "policy",
                },
              }
            : {}),
        },
      });
    // The Run's content is private to its owner, so the reader gets a plain refusal.
    return route.fulfill({ status: 403, json: { error: "forbidden" } });
  });
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Execution history", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByRole("button")
    .filter({ hasText: "Researcher" })
    .click();
  const panel = page.getByRole("region", { name: "Execution management" });
  await expect(
    panel.getByRole("button", { name: "Decline this call", exact: true }),
  ).toBeVisible();
  await expect(
    panel.getByRole("button", { name: /Pause|Cancel execution|Resume/ }),
  ).toHaveCount(0);
  await panel
    .getByRole("button", { name: "Decline this call", exact: true })
    .click();
  await expect(panel.getByText("task_delegate")).toBeVisible();
  // Someone who cannot read the Run cannot send it a message: no sentence field.
  await expect(panel.getByLabel("Tell the Agent why (optional)")).toHaveCount(
    0,
  );
  await panel
    .getByRole("button", { name: "Decline and continue", exact: true })
    .click();
  await expect.poll(() => declined).toEqual([{ call: "run-0:3:0" }]);
  await expect(
    panel.getByRole("button", { name: "Decline this call" }),
  ).toHaveCount(0);
  expect(errors).toEqual([]);
});

for (const failMessage of [false, true]) {
  test(`on another Node the call is declined first and the sentence follows${failMessage ? ", and a retry sends only the sentence" : ""}`, async ({
    page,
  }) => {
    const { errors } = await setup(page, { subject: true });
    const grant = {
      id: "4191ce62-a1a8-42d6-9926-bd32f5e0dc34",
      task_id: "task-0",
      node_id: "aidash://remote",
      agent: { id: "researcher", version: "1.0.0" },
      expires_at: new Date(Date.now() + 3600000).toISOString(),
      revoked: false,
    };
    const execution: Record<string, unknown> = {
      grant_id: grant.id,
      admission_id: "829cb342-d2c8-43e4-99c2-0dbe96bc4f70",
      run_id: "829cb342-d2c8-43e4-99c2-0dbe96bc4f70",
      phase: "THINKING",
      control: "PAUSED",
      error: null,
      declinable_call: {
        call: "829cb342-d2c8-43e4-99c2-0dbe96bc4f70:5:0",
        tool: "task_delegate",
        kind: "other_node",
      },
    };
    const semantic = {
      state: "ready",
      reason: null,
      operation_id: "c8cf392a-f1d4-42e1-8261-a003d1422660",
      retry_count: 0,
      retry_at: null,
      result_count: 0,
      truncated: false,
      retrieved_at: new Date().toISOString(),
      recovery: null,
    };
    const order: string[] = [];
    let failures = failMessage ? 1 : 0;
    await page.route("**/api/tasks/task-0/remote-**", async (route) => {
      const path = new URL(route.request().url()).pathname;
      if (path.endsWith("/remote-executions"))
        return route.fulfill({
          json: [{ grant, execution, semantic, unavailable: false }],
        });
      if (path.endsWith("/declined-calls")) {
        order.push("decline");
        execution.control = "ACTIVE";
        delete execution.declinable_call;
        return route.fulfill({ json: execution });
      }
      if (path.endsWith("/messages")) {
        order.push("message");
        if (failures-- > 0)
          return route.fulfill({
            status: 503,
            json: { error: { message: "receiver unavailable" } },
          });
        return route.fulfill({ json: { accepted: true } });
      }
      return route.fallback();
    });
    await page.goto("/collaboration?channel=workspace-one");
    await page
      .getByRole("button", { name: "Tasks and results", exact: true })
      .click();
    await page
      .getByRole("dialog")
      .getByRole("button")
      .filter({ hasText: "Collect evidence" })
      .click();
    const panel = page.getByRole("region", {
      name: "Remote execution",
      exact: true,
    });
    await panel
      .getByRole("button", { name: "Decline this call", exact: true })
      .click();
    // The hint says plainly that the sentence goes after the decline on another Node.
    await expect(panel).toContainText("sent after it");
    await panel
      .getByLabel("Tell the Agent why (optional)")
      .fill("Use the shared notes instead.");
    await panel
      .getByRole("button", { name: "Decline and continue", exact: true })
      .click();
    if (failMessage) {
      await expect(panel.getByRole("alert")).toContainText(
        "receiver unavailable",
      );
      expect(order).toEqual(["decline", "message"]);
      // The call is declined and the form stays open; a retry sends only the sentence.
      await expect(panel).toContainText("has not been sent yet");
      await panel
        .getByRole("button", { name: "Decline and continue", exact: true })
        .click();
    }
    await expect
      .poll(() => order)
      .toEqual(
        failMessage
          ? ["decline", "message", "message"]
          : ["decline", "message"],
      );
    expect(errors.filter((error) => !error.includes("503"))).toEqual([]);
  });
}

test("workspace history continues from its candidate cursor", async ({
  page,
}) => {
  await setup(page, { locale: "en-US" });
  const data = fixture();
  await page.route("**/api/state**", (route) =>
    route.fulfill({
      json: {
        ...data,
        access: { kind: "subject", tenant: "acme", subject: "alice" },
        next_cursors: { ...data.next_cursors, workspaces: "next-page" },
      },
    }),
  );
  let requested = "";
  await page.route("**/api/workspaces?**", (route) => {
    requested = route.request().url();
    return route.fulfill({
      json: {
        items: [
          {
            ...data.workspaces[0],
            id: "older-workspace",
            title: "Older workspace",
          },
        ],
        next_cursor: null,
      },
    });
  });
  await page.goto("/collaboration?channel=workspace-one");
  await page.getByRole("button", { name: /Switch request/ }).click();
  await page.getByRole("button", { name: "Show more" }).click();
  await expect(
    page.getByRole("link", { name: "# Older workspace" }),
  ).toBeVisible();
  expect(new URL(requested).searchParams.get("cursor")).toBe("next-page");
  expect(new URL(requested).searchParams.get("node")).toBe(defaultNode.name);
  await expect(page.getByRole("button", { name: "Show more" })).toHaveCount(0);
  await page.getByRole("link", { name: "# Older workspace" }).click();
  await expect(page).toHaveURL(/channel=older-workspace/);
  await expect(
    page.getByRole("heading", { name: "# Older workspace" }),
  ).toBeVisible();
});

test("pending requests continue within the selected node", async ({ page }) => {
  await setup(page, { locale: "en-US" });
  const data = fixture();
  await page.route("**/api/state**", (route) =>
    route.fulfill({
      json: {
        ...data,
        access: { kind: "subject", tenant: "acme", subject: "alice" },
        next_cursors: { ...data.next_cursors, human_requests: "next-human" },
      },
    }),
  );
  let requested = "";
  await page.route("**/api/human-requests?**", (route) => {
    requested = route.request().url();
    return route.fulfill({
      json: {
        items: [
          {
            ...data.human_requests[0],
            id: "older-question",
            run_id: "old-run",
            workspace_id: "older-workspace",
            prompt: "Older request from an old run",
          },
        ],
        next_cursor: null,
      },
    });
  });
  await page.route("**/api/workspaces/older-workspace", (route) =>
    route.fulfill({
      json: {
        workspace: {
          ...data.workspaces[0],
          id: "older-workspace",
          title: "Older workspace",
        },
        tasks: [],
        artifacts: [],
        events: [],
        messages: [],
      },
    }),
  );
  await page.goto("/collaboration?channel=workspace-one");
  await page.getByRole("button", { name: /Decisions 1/ }).click();
  expect(requested).toBe("");
  await page.getByRole("button", { name: "Show more" }).click();
  await expect(
    page.getByRole("button", {
      name: /Information requested.*Older request from an old run/,
    }),
  ).toBeVisible();
  expect(new URL(requested).searchParams.get("cursor")).toBe("next-human");
  expect(new URL(requested).searchParams.get("node")).toBe(defaultNode.name);
  await page
    .getByRole("button", {
      name: /Information requested.*Older request from an old run/,
    })
    .last()
    .click();
  await expect(page).toHaveURL(/channel=older-workspace/);
  await expect(page.getByRole("dialog", { name: "Human input" })).toContainText(
    "Older request from an old run",
  );
});

test("an empty pending-request page can continue", async ({ page }) => {
  await setup(page, { locale: "en-US" });
  const data = fixture();
  await page.route("**/api/state**", (route) =>
    route.fulfill({
      json: {
        ...data,
        human_requests: [],
        access: { kind: "subject", tenant: "acme", subject: "alice" },
        next_cursors: { ...data.next_cursors, human_requests: "hidden-page" },
      },
    }),
  );
  await page.route("**/api/human-requests?**", (route) =>
    route.fulfill({
      json: {
        items: [
          {
            ...data.human_requests[0],
            prompt: "Request beyond hidden candidates",
          },
        ],
        next_cursor: null,
      },
    }),
  );
  await page.goto("/collaboration?channel=workspace-one");
  await page.getByRole("button", { name: /Decisions 0/ }).click();
  await page.getByRole("button", { name: "Show more" }).click();
  await expect(
    page
      .getByRole("button", {
        name: /Information requested.*Request beyond hidden candidates/,
      })
      .last(),
  ).toBeVisible();
});

test("an opened older workspace survives a fresh state window", async ({
  page,
}) => {
  await setup(page, { locale: "en-US" });
  const data = fixture();
  await page.route("**/api/workspaces/older-workspace", (route) =>
    route.fulfill({
      json: {
        workspace: {
          ...data.workspaces[0],
          id: "older-workspace",
          title: "Older workspace",
        },
        tasks: [],
        artifacts: [],
        events: [],
        messages: [],
      },
    }),
  );
  await page.goto("/collaboration?channel=older-workspace");
  await expect(
    page.getByRole("heading", { name: "# Older workspace" }),
  ).toBeVisible();
});

test("task dependency picker can load older tasks on demand", async ({
  page,
}) => {
  await setup(page, { locale: "en-US" });
  const data = fixture();
  await page.route("**/api/state**", (route) =>
    route.fulfill({
      json: {
        ...data,
        access: { kind: "subject", tenant: "acme", subject: "alice" },
        next_cursors: { ...data.next_cursors, tasks: "next-task" },
      },
    }),
  );
  let reads = 0;
  await page.route("**/api/tasks?**", (route) => {
    reads += 1;
    return route.fulfill({
      json: {
        tasks: [
          { ...data.tasks[0], id: "old-task", title: "Older prerequisite" },
        ],
        next_offset: null,
      },
    });
  });
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Show channel status", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByRole("tab", { name: "Tasks and results", exact: true })
    .click();
  await page.getByRole("button", { name: "Add task", exact: true }).click();
  expect(reads).toBe(0);
  await page.getByRole("button", { name: "Show more tasks" }).click();
  await expect(
    page.getByRole("option", { name: "Older prerequisite" }).first(),
  ).toBeAttached();
  expect(reads).toBe(1);
  const dependencies = page.locator('select[name="dependencies"]');
  await dependencies.selectOption("old-task");
  const refreshed = page.waitForResponse(
    (response) => new URL(response.url()).pathname === "/api/state",
  );
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await refreshed;
  await expect(dependencies).toHaveValues(["old-task"]);
});

test("failed task can be abandoned even when older runs exist", async ({
  page,
}) => {
  await setup(page, { subject: true, locale: "en-US" });
  const data = fixture();
  const task = { ...data.tasks[0], status: "FAILED" };
  await page.route("**/api/state**", (route) =>
    route.fulfill({
      json: {
        ...data,
        tasks: [task],
        access: { kind: "subject", tenant: "acme", subject: "alice" },
        next_cursors: { ...data.next_cursors, runs: "older" },
      },
    }),
  );
  await page.route("**/api/workspaces/workspace-one", (route) =>
    route.fulfill({
      json: {
        workspace: data.workspaces[0],
        tasks: [task],
        artifacts: [],
        events: [],
        messages: [],
      },
    }),
  );
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Show channel status", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByRole("tab", { name: "Tasks and results", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByRole("button")
    .filter({ hasText: task.title })
    .click();
  await expect(page.getByRole("button", { name: /abandon/i })).toBeVisible();
});

test("returning focus refreshes the current state", async ({ page }) => {
  await setup(page, { locale: "en-US" });
  let reads = 0;
  page.on("request", (request) => {
    if (new URL(request.url()).pathname === "/api/state") reads += 1;
  });
  await page.goto("/collaboration?channel=workspace-one");
  await expect(
    page.getByRole("heading", { name: "# Research", exact: true }),
  ).toBeVisible();
  const before = reads;
  await page.evaluate(() => window.dispatchEvent(new Event("focus")));
  await expect.poll(() => reads).toBeGreaterThan(before);
});

test("a remote decline holder sees only the paused call and cannot send a sentence", async ({
  page,
}) => {
  const { errors } = await setup(page, { subject: true });
  const grant = {
    id: "grant-holder",
    node_id: "aidash://remote",
    agent: { id: "researcher", version: "1.0.0" },
  };
  const call = { call: "run:5:0", tool: "task_delegate", kind: "other_node" };
  const declines: unknown[] = [];
  await page.route("**/api/tasks/task-0/remote-**", async (route) => {
    const path = new URL(route.request().url()).pathname;
    if (path.endsWith("/remote-executions"))
      return route.fulfill({
        json: [
          {
            view: "holder",
            grant,
            execution: { control: "PAUSED", declinable_call: call },
          },
        ],
      });
    if (path.endsWith("/declined-calls")) {
      declines.push(route.request().postDataJSON());
      return route.fulfill({ json: {} });
    }
    return route.fallback();
  });
  await page.goto("/collaboration?channel=workspace-one");
  await page
    .getByRole("button", { name: "Tasks and results", exact: true })
    .click();
  await page
    .getByRole("dialog")
    .getByRole("button")
    .filter({ hasText: "Collect evidence" })
    .click();
  const panel = page.getByRole("region", {
    name: "Remote execution",
    exact: true,
  });
  await panel
    .getByRole("button", { name: "Decline this call", exact: true })
    .click();
  await expect(panel.getByLabel("Tell the Agent why (optional)")).toHaveCount(
    0,
  );
  await expect(
    panel.getByRole("button", { name: /Pause|Cancel execution|Resume/ }),
  ).toHaveCount(0);
  await panel
    .getByRole("button", { name: "Decline and continue", exact: true })
    .click();
  await expect.poll(() => declines).toEqual([{ call: call.call }]);
  expect(errors).toEqual([]);
});
