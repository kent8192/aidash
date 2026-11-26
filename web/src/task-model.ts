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
  model?: Pick<CatalogModel, "supported_parameters" | "reasoning">,
) {
  if (!model?.supported_parameters?.includes("reasoning_effort")) return [];
  const supported = model.reasoning?.supported_efforts;
  return efforts.filter((e) => !supported || supported.includes(e));
}

export function perMillion(price?: string) {
  if (!price?.trim()) return null;
  const value = Number(price);
  return Number.isFinite(value) && value >= 0 ? value * 1_000_000 : null;
}

export function catalogConfig(m: CatalogModel) {
  return {
    provider: "openrouter",
    provider_credential: "openrouter",
    model_id: m.id,
    endpoint: "https://openrouter.ai/api/v1",
    reasoning_effort: null,
    context_window: m.context_length,
    max_output_tokens: m.top_provider?.max_completion_tokens ?? 0,
    modalities: m.architecture.input_modalities,
    media_routes: [],
    request_estimator: "calibrated",
    cost: {
      input_per_million: perMillion(m.pricing.prompt),
      output_per_million: perMillion(m.pricing.completion),
      currency: "USD",
    },
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
        config: catalogConfig(m),
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
export type RemoteModel = Pick<
  CatalogModel,
  "id" | "name" | "pricing" | "supported_parameters" | "reasoning"
> & {
  model: EntityRef;
  reasoning_effort?: Effort;
  created?: number;
};
export function receivingModels(
  task: string,
  node_id: string,
  agent: EntityRef,
) {
  return apiFetch<{ task_model_required: boolean; models: RemoteModel[] }>(
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
