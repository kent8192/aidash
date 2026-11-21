/** Duty templates in the Marketplace (#1011 D4): what the API returns for a
 *  template and the pure rules the publish and install dialogs share. A
 *  template carries a Duty's work, never its Requester or a secret. */
import type { Script, Trigger, Watcher } from "./duties-model";

/** The Marketplace package kind of a Duty template. */
export const DUTY_TEMPLATE_KIND = "duty_template";
/** The longest instruction for one trigger, as the server allows. */
export const INSTRUCTION_MAX = 2_000;
/** The longest description a publisher writes. */
export const DESCRIPTION_MAX = 1_000;

/** A Duty Trigger a template expects, with what to set up. A time trigger
 *  is set up as given; the others are the installing person's to create. */
export type ExpectedTrigger =
  | {
      kind: "cron";
      expression: string;
      time_zone: string;
      instruction: string;
    }
  | { kind: "interval"; every_seconds: number; instruction: string }
  | { kind: "webhook"; instruction: string }
  | { kind: "duty_completed"; instruction: string }
  | { kind: "node_event"; instruction: string };
/** Which trigger of a Duty an instruction is about. */
export type TriggerSlot = "time" | "webhook" | "duty_completed" | "node_event";
export type DutyTemplate = {
  title: string;
  mission: string;
  procedure: string;
  router?: Script | null;
  watcher?: Watcher | null;
  triggers: ExpectedTrigger[];
  agent?: { id: string; version: string } | null;
};
/** The template version a Duty was installed from. */
export type TemplateSource = {
  key: string;
  repository: string;
  owner_tenant: string;
  package_id: string;
  version: string;
};
export type AgentRef = { id: string; version: string };

export function isDutyTemplate(kind: string): boolean {
  return kind === DUTY_TEMPLATE_KIND;
}

/** The template a Marketplace manifest of a Duty template carries. */
export function templateOf(manifest: {
  entity: { kind: string; config?: unknown };
}): DutyTemplate | undefined {
  if (!isDutyTemplate(manifest.entity.kind)) return undefined;
  const config = manifest.entity.config as Partial<DutyTemplate> | undefined;
  if (!config || typeof config.mission !== "string") return undefined;
  return {
    title: String(config.title ?? ""),
    mission: config.mission,
    procedure: String(config.procedure ?? ""),
    router: config.router ?? null,
    watcher: config.watcher ?? null,
    triggers: Array.isArray(config.triggers) ? config.triggers : [],
    agent: config.agent ?? null,
  };
}

export function slotOf(trigger: ExpectedTrigger): TriggerSlot {
  return trigger.kind === "cron" || trigger.kind === "interval"
    ? "time"
    : trigger.kind;
}

/** The time trigger a template sets up, if any. */
export function timeTriggerOf(template: DutyTemplate): Trigger | null {
  for (const trigger of template.triggers) {
    if (trigger.kind === "cron")
      return {
        kind: "cron",
        expression: trigger.expression,
        time_zone: trigger.time_zone,
      };
    if (trigger.kind === "interval")
      return { kind: "interval", every_seconds: trigger.every_seconds };
  }
  return null;
}

/** The triggers the installing person creates on the Duty themselves. */
export function toCreate(template: DutyTemplate): ExpectedTrigger[] {
  return template.triggers.filter((trigger) => slotOf(trigger) !== "time");
}

/** The Agents the person may staff a Duty with: the readable ones, narrowed
 *  to those approved on the Node when the approvals could be read. */
export function agentChoices(
  agents: AgentRef[],
  approvals?: { entry_id: string; entry_version: string; enabled: boolean }[],
): AgentRef[] {
  if (!approvals) return agents;
  return agents.filter((agent) =>
    approvals.some(
      (approval) =>
        approval.enabled &&
        approval.entry_id === agent.id &&
        approval.entry_version === agent.version,
    ),
  );
}

/** The suggested Agent when the Node approved it, else none: the person
 *  then picks one. */
export function defaultAgent(
  template: DutyTemplate,
  choices: AgentRef[],
): AgentRef | undefined {
  const suggested = template.agent;
  if (!suggested) return undefined;
  return choices.find(
    (agent) => agent.id === suggested.id && agent.version === suggested.version,
  );
}

export function agentKey(agent: AgentRef): string {
  return `${agent.id}@${agent.version}`;
}

/** What the publish dialog edits. */
export type PublishForm = {
  packageId: string;
  version: string;
  author: string;
  description: string;
  suggestAgent: boolean;
  /** The instruction for each trigger of the Duty, by slot. */
  instructions: Partial<Record<TriggerSlot, string>>;
};
export type PublishProblem =
  | "package_id"
  | "version"
  | "author"
  | "description"
  | "instruction";

/** A package id from a Duty's title: lower case words joined by hyphens. */
export function packageIdOf(title: string): string {
  return (
    title
      .toLowerCase()
      .normalize("NFKD")
      .replace(/[^a-z0-9]+/g, "-")
      .replace(/^-+|-+$/g, "")
      .slice(0, 64) || "duty-template"
  );
}

/** The next minor version after `version`, for publishing again. */
export function nextVersion(version?: string): string {
  const match = /^(\d+)\.(\d+)\.(\d+)$/.exec(version ?? "");
  if (!match) return "1.0.0";
  return `${match[1]}.${Number(match[2]) + 1}.0`;
}

export function publishForm(
  draft: DutyTemplate,
  previous?: TemplateSource | null,
): PublishForm {
  const instructions: PublishForm["instructions"] = {};
  for (const trigger of draft.triggers)
    instructions[slotOf(trigger)] = trigger.instruction;
  return {
    packageId: previous?.package_id ?? packageIdOf(draft.title),
    version: nextVersion(previous?.version),
    author: "",
    description: "",
    suggestAgent: true,
    instructions,
  };
}

const SEMVER = /^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

export function publishProblems(
  form: PublishForm,
  draft: DutyTemplate,
): PublishProblem[] {
  const problems: PublishProblem[] = [];
  const id = form.packageId.trim();
  if (!id || id.length > 256 || /[\s*]/.test(id)) problems.push("package_id");
  if (!SEMVER.test(form.version.trim())) problems.push("version");
  if (!form.author.trim() || form.author.length > 256) problems.push("author");
  if (form.description.trim().length > DESCRIPTION_MAX)
    problems.push("description");
  if (
    draft.triggers.some((trigger) => {
      const text = (form.instructions[slotOf(trigger)] ?? "").trim();
      return !text || text.length > INSTRUCTION_MAX;
    })
  )
    problems.push("instruction");
  return problems;
}

/** The body of `POST /api/duties/{id}/template`. */
export function publishBody(
  form: PublishForm,
  draft: DutyTemplate,
  idempotencyKey: string,
) {
  return {
    package_id: form.packageId.trim(),
    version: form.version.trim(),
    author: form.author.trim(),
    description: form.description.trim(),
    triggers: draft.triggers.map((trigger) => ({
      kind: slotOf(trigger),
      instruction: (form.instructions[slotOf(trigger)] ?? "").trim(),
    })),
    suggest_agent: form.suggestAgent,
    idempotency_key: idempotencyKey,
  };
}

/** The body of `POST /api/marketplace/packages/{key}/duty`. */
export function installBody(
  digest: string,
  threadId: string,
  agent: AgentRef | undefined,
  idempotencyKey: string,
) {
  return {
    digest,
    thread_id: threadId,
    ...(agent ? { agent: { id: agent.id, version: agent.version } } : {}),
    idempotency_key: idempotencyKey,
  };
}

/** What a later template version changes in a Duty: the fields an update
 *  replaces, each as the text the person compares. */
export type UpdateField = "mission" | "procedure" | "router" | "watcher";
export type UpdateChange = {
  field: UpdateField;
  before: string;
  after: string;
};
/** One line of a line diff: kept, removed or added. */
export type DiffLine = { kind: " " | "-" | "+"; text: string };

function scriptText(script?: Script | null): string {
  return script ? `${script.language}\n${script.source}` : "";
}

function watcherText(watcher?: Watcher | null): string {
  return watcher
    ? `every ${watcher.every_seconds} s\n${scriptText(watcher.script)}`
    : "";
}

/** The fields a template update would replace in `duty` and how, in the
 *  order the update dialog shows them. Unchanged fields are left out. An
 *  update replaces the mission, procedure and scripts; it keeps the
 *  Duty's title, Agent and triggers (#1011 D4). */
export function updateChanges(
  duty: {
    mission: string;
    procedure: string;
    router?: Script | null;
    watcher?: Watcher | null;
  },
  template: DutyTemplate,
): UpdateChange[] {
  const fields: [UpdateField, string, string][] = [
    ["mission", duty.mission, template.mission],
    ["procedure", duty.procedure, template.procedure],
    ["router", scriptText(duty.router), scriptText(template.router)],
    ["watcher", watcherText(duty.watcher), watcherText(template.watcher)],
  ];
  return fields
    .filter(([, before, after]) => before !== after)
    .map(([field, before, after]) => ({ field, before, after }));
}

/** A line diff of `before` and `after` (longest common subsequence). */
export function lineDiff(before: string, after: string): DiffLine[] {
  const a = before === "" ? [] : before.split("\n");
  const b = after === "" ? [] : after.split("\n");
  const common = Array.from({ length: a.length + 1 }, () =>
    new Array<number>(b.length + 1).fill(0),
  );
  for (let i = a.length - 1; i >= 0; i--)
    for (let j = b.length - 1; j >= 0; j--)
      common[i][j] =
        a[i] === b[j]
          ? common[i + 1][j + 1] + 1
          : Math.max(common[i + 1][j], common[i][j + 1]);
  const lines: DiffLine[] = [];
  let i = 0;
  let j = 0;
  while (i < a.length || j < b.length) {
    if (i < a.length && j < b.length && a[i] === b[j]) {
      lines.push({ kind: " ", text: a[i] });
      i++;
      j++;
    } else if (
      j < b.length &&
      (i === a.length || common[i][j + 1] >= common[i + 1][j])
    ) {
      lines.push({ kind: "+", text: b[j] });
      j++;
    } else {
      lines.push({ kind: "-", text: a[i] });
      i++;
    }
  }
  return lines;
}

/** The body of `POST /api/duties/{id}/template-update`: the version the
 *  person reviewed, by its key and the digest of what they were shown. The
 *  server applies exactly that version and refuses a digest that is not
 *  its own, so nothing is applied that the person did not see. */
export function updateBody(key: string, reviewedDigest: string) {
  return { key, digest: reviewedDigest };
}
