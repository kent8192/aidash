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
