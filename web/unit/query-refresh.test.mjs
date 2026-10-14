import { test } from "node:test";
import assert from "node:assert/strict";
import { coalescedRefresh } from "../src/query-refresh.ts";

/** A query client whose fetches settle only when the test releases them. */
function fakeClient() {
  const fetches = [];
  let inFlight = null;
  return {
    fetches,
    options: [],
    isFetching: () => (inFlight ? 1 : 0),
    invalidateQueries(_filters, options) {
      this.options.push(options);
      // Without cancellation an in-flight fetch is reused, as TanStack does.
      if (inFlight) return inFlight.promise;
      let resolve;
      const promise = new Promise((done) => (resolve = done));
      inFlight = { promise };
      fetches.push(() => {
        inFlight = null;
        resolve();
      });
      return promise;
    },
    startExternalFetch() {
      return this.invalidateQueries({ queryKey: ["state"] }, {});
    },
  };
}
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

test("never cancels an in-flight refresh", async () => {
  const client = fakeClient();
  const { refresh } = coalescedRefresh(client);
  refresh("state");
  refresh("state");
  refresh("state");
  assert.equal(client.fetches.length, 1);
  assert.ok(client.options.every((option) => option.cancelRefetch === false));
});

test("events during a fetch earn exactly one trailing refresh", async () => {
  const client = fakeClient();
  const { refresh } = coalescedRefresh(client);
  refresh("state");
  refresh("state");
  refresh("state");
  client.fetches[0]();
  await settle();
  assert.equal(client.fetches.length, 2, "one trailing refresh");
  client.fetches[1]();
  await settle();
  assert.equal(client.fetches.length, 2, "no refresh without a new event");
});

test("an event overlapping a fetch it did not start refreshes again", async () => {
  const client = fakeClient();
  const { refresh } = coalescedRefresh(client);
  void client.startExternalFetch();
  refresh("state");
  assert.equal(client.fetches.length, 1, "the external fetch is reused");
  client.fetches[0]();
  await settle();
  assert.equal(client.fetches.length, 2, "its response may predate the event");
});

test("close stops pending trailing refreshes", async () => {
  const client = fakeClient();
  const { refresh, close } = coalescedRefresh(client);
  refresh("state");
  refresh("state");
  close();
  client.fetches[0]();
  await settle();
  assert.equal(client.fetches.length, 1);
});

test("a fetch it cannot wait on earns at most one trailing refresh", async () => {
  // A disabled query can be active and fetching while `invalidateQueries`
  // skips it and settles at once; overlap stays true until the fetch lands.
  const calls = [];
  const client = {
    isFetching: (filters) => (filters.type === "active" ? 1 : 0),
    invalidateQueries() {
      calls.push(null);
      if (calls.length > 10) throw new Error("refresh loop");
      return Promise.resolve();
    },
  };
  const { refresh } = coalescedRefresh(client);
  refresh("state");
  await settle();
  assert.equal(calls.length, 2);
});

test("only active fetches count as overlap", async () => {
  // `invalidateQueries` refetches only active queries, so an inactive fetch
  // is never waited on and needs no trailing refresh.
  const calls = [];
  const client = {
    isFetching: (filters) => (filters.type === "active" ? 0 : 1),
    invalidateQueries() {
      calls.push(null);
      if (calls.length > 10) throw new Error("refresh loop");
      return Promise.resolve();
    },
  };
  const { refresh } = coalescedRefresh(client);
  refresh("state");
  await settle();
  assert.equal(calls.length, 1);
});
