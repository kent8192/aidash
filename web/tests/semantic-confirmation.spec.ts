import { expect, test } from "@playwright/test";
import { setup } from "./collaboration-fixture";

for (const action of ["deletes", "recomputes"] as const) {
  test(`semantic configuration confirms before it ${action} memory embeddings`, async ({
    page,
  }) => {
    await setup(page, { locale: "en-US" });
    const thread = "thread-one";
    const root = `**/api/threads/${thread}/semantic`;
    let index = {
      thread_id: thread,
      tenant: "acme",
      revision: 4,
      collection: "fixture",
      updated_at: "2026-11-25T00:00:00Z",
      spec: {
        enabled: true,
        auto_context: false,
        embedding: {
          provider: "openai_compatible",
          endpoint: "https://embedding.example/v1",
          model: "fixture",
          model_version: "v1",
          dimensions: 3,
          inputs: ["text"],
        },
        vector: { provider: "postgres", endpoint: "local" },
        max_sources: 64,
        max_results: 4,
        max_result_tokens: 4096,
        max_input_bytes: 8192,
        max_images: 256,
        max_image_bytes: 8388608,
      },
    };
    const requests: {
      expected_revision: number;
      spec: typeof index.spec;
      acknowledge_memory_embeddings?: boolean;
    }[] = [];
    await page.route(`${root}/entries`, (route) =>
      route.fulfill({ json: { entries: [], unconfirmed: [] } }),
    );
    await page.route(`${root}/history`, (route) => route.fulfill({ json: [] }));
    await page.route(`${root}/cleanup`, (route) =>
      route.fulfill({
        json: {
          points: { pending: 0, failed: 0 },
          collections: { pending: 0, failed: 0 },
        },
      }),
    );
    await page.route(`${root}/index`, (route) => {
      if (route.request().method() === "POST") {
        const draft: (typeof requests)[number] = route.request().postDataJSON();
        requests.push(draft);
        if (!draft.acknowledge_memory_embeddings) {
          return route.fulfill({
            status: 409,
            json: {
              error: `this ${action} the embeddings of 2 memory facts in this Thread; resend with acknowledge_memory_embeddings to continue`,
            },
          });
        }
        index = { ...index, revision: index.revision + 1, spec: draft.spec };
      }
      return route.fulfill({ json: index });
    });

    await page.goto("/semantic");
    await expect(page.locator(".semantic-summary").first()).toContainText(
      "Revision 4",
    );
    await page.getByRole("button", { name: "Configure index" }).click();
    if (action === "deletes") {
      await page.getByLabel("Enable index", { exact: true }).uncheck();
    } else {
      await page.getByLabel("Model version", { exact: true }).fill("v2");
    }

    const modal = page.getByRole("dialog");
    const save = modal.getByRole("button", { name: "Save" });
    page.once("dialog", async (dialog) => {
      expect(dialog.message()).toContain(
        `${action} the embeddings of 2 memory facts`,
      );
      await dialog.dismiss();
    });
    await save.click();
    await expect(save).toBeEnabled();
    expect(requests).toHaveLength(1);
    expect(requests[0].expected_revision).toBe(4);
    expect(requests[0].acknowledge_memory_embeddings).toBeUndefined();
    expect(requests[0].spec.enabled).toBe(action !== "deletes");
    expect(requests[0].spec.embedding.model_version).toBe(
      action === "recomputes" ? "v2" : "v1",
    );

    page.once("dialog", (dialog) => dialog.accept());
    await save.click();
    await expect(modal).toHaveCount(0);
    expect(requests).toEqual([
      requests[0],
      requests[0],
      { ...requests[0], acknowledge_memory_embeddings: true },
    ]);
  });
}
