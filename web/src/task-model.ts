import { registryCreate } from "./generated/aidash";
import type { CatalogModel, EntityRef, Entry } from "./generated/models";
import { apiFetch } from "./transport";

export const efforts = ["low", "medium", "high", "xhigh", "max"] as const;
export type Effort = (typeof efforts)[number];
export type TaskModel = { model: EntityRef; reasoning_effort?: Effort };
export type TaskModelFields = { task_model?: TaskModel };
export type ModelChoice = {
  catalog?: CatalogModel;
  entry?: EntityRef;
  effort?: Effort;
  registrationKey: string;
};

export function allowedEfforts(
  model?: Partial<Pick<CatalogModel, "supported_parameters" | "reasoning">>,
) {
  if (!model?.supported_parameters?.includes("reasoning_effort")) return [];
  const supported = model.reasoning?.supported_efforts;
  return efforts.filter((e) => !supported || supported.includes(e));
}

export function defaultEffort(levels: Effort[], preferred: unknown) {
  return (
    levels.find((e) => e === preferred) ??
    levels.find((e) => e === "medium") ??
    levels[0]
  );
}

export function perMillion(price?: string) {
  if (!price?.trim()) return null;
  const value = Number(price);
  return Number.isFinite(value) && value >= 0 ? value * 1_000_000 : null;
}

export function modelDefinitionConfig(
  source: string,
  model: {
    model_id: string;
    endpoint: string;
    reasoning_effort: string | null;
    context_window: number;
    max_output_tokens: number;
    modalities: unknown;
    media_routes: unknown;
    cost: unknown;
  },
  wireApi = "",
  cacheMode = "",
  encoding = "",
) {
  return {
    ...model,
    provider: source === "keyless" ? "openrouter" : source,
    ...(source === "chatgpt"
      ? { plan_connection: "chatgpt", streaming: true }
      : {}),
    ...(source === "openrouter"
      ? {
          provider_credential: "openrouter",
          ...(wireApi === "responses" ? { wire_api: "responses" } : {}),
          ...(cacheMode ? { cache_mode: cacheMode } : {}),
        }
      : {}),
    request_estimator: encoding ? { tokenized: encoding } : "calibrated",
  };
}
export async function resolveChoice(choice: ModelChoice): Promise<TaskModel> {
  let ref = choice.entry;
  if (!ref) {
    const m = choice.catalog!;
    const created = await registryCreate(
      {
        kind: "model",
        id: "",
        version: "1.0.0",
        name: { en: m.name },
        description: { en: m.id },
        config: modelDefinitionConfig("openrouter", {
          model_id: m.id,
          endpoint: "https://openrouter.ai/api/v1",
          reasoning_effort: choice.effort ?? null,
          context_window: m.context_length,
          max_output_tokens: m.top_provider?.max_completion_tokens ?? 0,
          modalities: m.architecture.input_modalities,
          media_routes: [],
          cost: {
            input_per_million: perMillion(m.pricing.prompt),
            output_per_million: perMillion(m.pricing.completion),
            currency: "USD",
          },
        }),
        tags: [],
        languages: [],
        capabilities: [],
        skills: [],
        schema: {},
      },
      { headers: { "Idempotency-Key": choice.registrationKey } },
    );
    ref = { id: created.id, version: created.version };
  }
  return {
    model: { id: ref.id, version: ref.version },
    reasoning_effort: choice.effort,
  };
}

export function needsTaskModel(agent?: Pick<Entry, "kind" | "config">) {
  return agent?.kind === "agent" && agent.config?.model == null;
}

// Adapter for the receiver's discovery-scoped model list. Replace with the generated client.
type RemoteModel = {
  model: EntityRef;
  name: Entry["name"];
  openrouter_model_id: string;
  pricing?: CatalogModel["pricing"] | null;
  reasoning_effort?: Effort;
};
export function receivingModels(
  task: string,
  node_id: string,
  agent: EntityRef,
) {
  return apiFetch<RemoteModel[]>(
    `/api/tasks/${encodeURIComponent(task)}/remote-grants/models`,
    {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({
        node_id,
        agent: { id: agent.id, version: agent.version },
      }),
    },
  );
}
