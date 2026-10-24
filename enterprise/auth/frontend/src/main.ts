import init, { mount } from "../pkg/aidash_auth_web.js";
import type {
  GcipClient,
  GcipClientConfig,
  GcipClientOptions,
} from "./gcip-sdk";
import "./style.css";

declare global {
  var aidashCreateGcipClient: (
    config: GcipClientConfig,
    options?: GcipClientOptions,
  ) => Promise<GcipClient>;
}

globalThis.aidashCreateGcipClient = async (config, options) => {
  const { createClient } = await import("./gcip-sdk");
  return createClient(config, options);
};

async function start() {
  await init();
  mount();
}

void start();
