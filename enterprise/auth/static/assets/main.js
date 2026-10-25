// Only the upstream GCIP SDK uses browser JavaScript. UI and RPC are Pages.
globalThis.aidashCreateGcipClient = async (config, options) => {
  const { createClient } = await import("./gcip-sdk.js");
  return createClient(config, options);
};
import init from "./aidash_auth_server.js";
await init();
