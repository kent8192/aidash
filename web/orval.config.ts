import { defineConfig } from "orval";
export default defineConfig({
  aidash: {
    input: { target: "../openapi/aidash.json" },
    output: {
      target: "src/generated/aidash.ts",
      schemas: "src/generated/models",
      client: "fetch",
      clean: true,
      override: {
        fetch: { includeHttpResponseReturnType: false },
        mutator: { path: "src/transport.ts", name: "apiFetch" },
        // SSE is an unbounded response: expose its reader instead of buffering text.
        transformer: (operation) =>
          operation.response.contentTypes.includes("text/event-stream")
            ? {
                ...operation,
                response: {
                  ...operation.response,
                  definition: {
                    ...operation.response.definition,
                    success: "Response",
                  },
                },
              }
            : operation,
      },
    },
    hooks: {
      afterAllFilesWrite:
        "prettier --write src/generated ../openapi/aidash.json",
    },
  },
});
