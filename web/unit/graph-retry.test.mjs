import assert from "node:assert/strict";
import test from "node:test";
import { retryFreshGraphPage } from "../src/collaboration/graph-retry.ts";

const conflict = () => Object.assign(new Error("Graph changed"), { status: 409 });

test("a fresh page retries transient generation conflicts", async () => {
  const page = { nodes: [{ id: "current" }] };
  let calls = 0;
  const result = await retryFreshGraphPage(null, async () => {
    if (++calls < 3) throw conflict();
    return page;
  });
  assert.equal(result, page);
  assert.equal(calls, 3);
});

test("persistent fresh-page conflicts stop after three attempts", async () => {
  const error = conflict();
  let calls = 0;
  await assert.rejects(
    retryFreshGraphPage(null, async () => {
      calls++;
      throw error;
    }),
    (actual) => actual === error,
  );
  assert.equal(calls, 3);
});

test("a continuation conflict returns immediately for the caller's restart", async () => {
  const error = conflict();
  let calls = 0;
  await assert.rejects(
    retryFreshGraphPage("bound-cursor", async () => {
      calls++;
      throw error;
    }),
    (actual) => actual === error,
  );
  assert.equal(calls, 1);
});

for (const status of [400, 401, 403, 404, 500]) {
  test(`fresh pages never retry HTTP ${status}`, async () => {
    const error = Object.assign(new Error("Request failed"), { status });
    let calls = 0;
    await assert.rejects(
      retryFreshGraphPage(null, async () => {
        calls++;
        throw error;
      }),
      (actual) => actual === error,
    );
    assert.equal(calls, 1);
  });
}

test("successful fresh and continuation pages are fetched only once", async () => {
  for (const cursor of [null, "bound-cursor"]) {
    let calls = 0;
    const page = await retryFreshGraphPage(cursor, async () => ++calls);
    assert.equal(page, 1);
    assert.equal(calls, 1);
  }
});

test("non-HTTP failures are not retried", async () => {
  const error = new Error("Network failed");
  let calls = 0;
  await assert.rejects(
    retryFreshGraphPage(null, async () => {
      calls++;
      throw error;
    }),
    (actual) => actual === error,
  );
  assert.equal(calls, 1);
});
