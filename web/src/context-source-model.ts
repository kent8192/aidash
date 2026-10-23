/** The opt-in Registry source declaration. Exact references pin dependencies;
 * declarations never confer read or disclosure permission. */
export type QualifiedSourceRef = {
  registry_deployment: string;
  id: string;
  version: string;
};
export type ContextSourceDeclaration = {
  provider: string;
  operation: string;
  tools: {
    read: QualifiedSourceRef[];
    write: QualifiedSourceRef[];
    detail: QualifiedSourceRef | null;
  };
  pin:
    | { policy: "always" | "on_demand" }
    | {
        policy: "scored";
        mode: "shadow" | "enforce";
        decider: QualifiedSourceRef;
      };
  volatility: "run" | "step";
  query: "task-inputs@1" | "accepted-inputs-first@1";
  refresh: "revision-bound@1";
  disclosure: { previews: boolean; query: boolean };
  required_home: boolean;
};
function exact(value: string): QualifiedSourceRef {
  const parsed: unknown = JSON.parse(value);
  if (!parsed || typeof parsed !== "object")
    throw new Error("Choose an exact Registry version");
  const ref = parsed as QualifiedSourceRef;
  if (
    ![ref.registry_deployment, ref.id, ref.version].every(
      (part) => typeof part === "string" && part.length > 0,
    )
  )
    throw new Error("Choose an exact Registry version");
  return {
    registry_deployment: ref.registry_deployment,
    id: ref.id,
    version: ref.version,
  };
}
/** Read only the opt-in controls, preserving existing source bytes by default. */
export function contextSourceConfiguration(
  form: FormData,
): ContextSourceDeclaration | undefined {
  if (!form.has("source_declaration")) return undefined;
  const text = (name: string) => String(form.get(name) ?? "");
  const policy = text("source_pin");
  const mode = text("source_mode");
  const required_home = form.has("source_required_home");
  if (!["always", "on_demand", "scored"].includes(policy))
    throw new Error("Choose a source pin policy");
  if (
    policy === "scored" &&
    (!["shadow", "enforce"].includes(mode) ||
      (required_home && mode === "enforce"))
  )
    throw new Error("Required-Home sources cannot enforce Scored selection");
  if (
    policy === "scored" &&
    (!form.has("source_disclose_previews") ||
      !form.has("source_disclose_query"))
  )
    throw new Error(
      "Scored selection requires explicit preview and query disclosure",
    );
  const provider = text("source_provider").trim();
  const operation = text("source_operation").trim();
  if (!provider || !operation)
    throw new Error("Enter the source provider and operation");
  return {
    provider,
    operation,
    tools: {
      read: form.getAll("source_read").map((value) => exact(String(value))),
      write: form.getAll("source_write").map((value) => exact(String(value))),
      detail: text("source_detail") ? exact(text("source_detail")) : null,
    },
    pin:
      policy === "scored"
        ? {
            policy,
            mode: mode as "shadow" | "enforce",
            decider: exact(text("source_decider")),
          }
        : { policy: policy as "always" | "on_demand" },
    volatility: text("source_volatility") === "run" ? "run" : "step",
    query:
      text("source_query") === "accepted-inputs-first@1"
        ? "accepted-inputs-first@1"
        : "task-inputs@1",
    refresh: "revision-bound@1",
    disclosure: {
      previews: form.has("source_disclose_previews"),
      query: form.has("source_disclose_query"),
    },
    required_home,
  };
}
