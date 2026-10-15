import { test } from "node:test";
import assert from "node:assert/strict";
import { keylessOrigin } from "../src/keyless.ts";

test("model and embedding definitions without a Provider Credential are keyless", () => {
  assert.equal(
    keylessOrigin({
      kind: "model",
      config: { provider: "openrouter", endpoint: "http://127.0.0.1:1/v1" },
    }),
    "http://127.0.0.1:1",
  );
  assert.equal(
    keylessOrigin({
      kind: "embedding",
      config: {
        provider: "openai",
        endpoint: "https://Embeddings.Example:443/v1",
      },
    }),
    "https://embeddings.example",
  );
});

test("Provider Catalog definitions and other kinds carry no notice", () => {
  assert.equal(
    keylessOrigin({
      kind: "model",
      config: {
        provider: "openrouter",
        endpoint: "https://openrouter.ai/api/v1",
        provider_credential: "openrouter",
      },
    }),
    null,
  );
  assert.equal(
    keylessOrigin({
      kind: "tool",
      config: { endpoint: "http://127.0.0.1:9/effect" },
    }),
    null,
  );
});

test("definitions without a usable HTTP endpoint carry no notice", () => {
  for (const config of [
    null,
    "openrouter",
    {},
    { endpoint: 42 },
    { endpoint: "not a url" },
    { endpoint: "file:///etc/passwd" },
  ])
    assert.equal(keylessOrigin({ kind: "model", config }), null);
});
