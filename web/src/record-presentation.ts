// Presentation of application-owned records only. User-authored payloads remain verbatim.
const payloadFields = new Set([
  "content",
  "text",
  "state",
  "attributes",
  "metadata",
  "input",
  "result",
  "instructions",
  "description",
  "schema",
  "permissions",
  "context",
  "memory",
]);
const primaryKeys = new Set([
  "id",
  "pk",
  "sequence",
  "idempotency_key",
  "digest",
]);
const references = new Set([
  "workspace_id",
  "task_id",
  "run_id",
  "entry_id",
  "agent_id",
  "credential_id",
  "policy_id",
  "request_id",
  "transaction_id",
  "message_id",
  "artifact_id",
  "attachment_id",
  "parent_id",
  "node_id",
  "home_node",
  "source_node",
  "coordinator",
]);
export function presentRecord(
  value: unknown,
  labels: ReadonlyMap<string, string>,
  unavailable: string,
): unknown {
  const label = (id: string, version?: unknown) =>
    labels.get(typeof version === "string" ? `${id}@${version}` : id) ??
    unavailable;
  const visit = (value: unknown): unknown => {
    if (Array.isArray(value)) return value.map(visit);
    if (typeof value === "string") {
      if (labels.has(value)) return labels.get(value);
      if (
        /^[0-9a-f]{8}(?:-[0-9a-f]{4}){3}-[0-9a-f]{12}$/i.test(value) ||
        /^aidash:\/\//.test(value)
      )
        return unavailable;
      return value;
    }
    if (!value || typeof value !== "object") return value;
    const record = value as Record<string, unknown>;
    const keys = Object.keys(record);
    if (
      typeof record.id === "string" &&
      keys.every((key) => ["id", "version", "kind"].includes(key))
    )
      return label(record.id, record.version);
    return Object.fromEntries(
      Object.entries(record).flatMap(([key, item]) => {
        if (primaryKeys.has(key)) return [];
        if (payloadFields.has(key)) return [[key, item]];
        if (references.has(key) && typeof item === "string") {
          const prefix = key.replace(/_id$/, "");
          return [[prefix, label(item, record[`${prefix}_version`])]];
        }
        return [[key, visit(item)]];
      }),
    );
  };
  return visit(value);
}
