import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Textarea } from "./components/ui/textarea";
import { useState } from "react";
import { Check, Group, Hint, pairClass } from "./components/patterns";
import { Field, useI18n } from "./ui";
import { disambiguateLabels } from "./display-labels";
import { hasPluginSkills, type AgentPluginRef } from "./agent-plugins";
import { groupByKind } from "./registry-kinds";
export type Binding = {
  kind: "tool" | "bundle" | "skill" | "memory" | "source" | "external_service";
  target: { registry_node: string; id: string; version: string };
  alias?: string;
  narrow: {
    allowed_hosts?: string[];
    scope?: Record<string, string[]>;
    limits?: Record<string, number>;
  };
  members?: string[];
};
export type BindingConfiguration = {
  bindings: Binding[];
  remove_default: string[];
};
export function hasInstructionalBinding(
  bindings: Binding[],
  entries: {
    id: string;
    version: string;
    kind: string;
    config: Record<string, unknown>;
  }[],
  node: string,
  plugins: readonly AgentPluginRef[] = [],
): boolean {
  if (hasPluginSkills(plugins)) return true;
  return bindings.some((binding) => {
    if (binding.target.registry_node !== node) return false;
    const entry = entries.find(
      (entry) =>
        entry.id === binding.target.id &&
        entry.version === binding.target.version &&
        entry.kind === binding.kind,
    );
    if (!entry) return false;
    if (binding.kind === "skill") return true;
    const source = entry.config.source as { adapter?: string } | undefined;
    return (
      binding.kind === "source" &&
      ["skill_attachments", "skill_roots"].includes(source?.adapter ?? "")
    );
  });
}
export const defaults = [
  "workspace_observe",
  "workspace_wait",
  "skill_list",
  "skill_load",
  "skill_read",
  "file_search",
  "file_read",
  "task_create",
  "task_delegate",
  "agent_discover",
  "artifact_publish",
  "workspace_message",
  "memory_mutate",
  "memory_recall",
  "memory_reflect",
];
export const coordinatorDefaults = [
  "task_create",
  "task_delegate",
  "agent_discover",
];
function Restrictions({
  id,
  value,
  change,
  ja,
}: {
  id?: string;
  value: Binding["narrow"];
  change: (value: Binding["narrow"]) => void;
  ja: boolean;
}) {
  const serialized = JSON.stringify(value ?? {});
  const [draft, setDraft] = useState({ source: serialized, text: serialized });
  if (draft.source !== serialized) {
    setDraft({ source: serialized, text: serialized });
  }
  return (
    <Textarea
      className="min-h-16 font-mono text-xs"
      id={id}
      value={draft.source === serialized ? draft.text : serialized}
      onChange={(e) => setDraft({ source: serialized, text: e.target.value })}
      onBlur={(e) => {
        try {
          const narrow = JSON.parse(e.target.value);
          e.target.setCustomValidity("");
          change(narrow);
        } catch {
          e.target.setCustomValidity(
            ja ? "JSONを入力してください" : "Enter valid JSON",
          );
          e.target.reportValidity();
        }
      }}
    />
  );
}
export function AgentBindings({
  value,
  change,
  node = "",
  entries = [],
  cluster = false,
}: {
  value: BindingConfiguration;
  change: (v: BindingConfiguration) => void;
  node?: string;
  cluster?: boolean;
  entries?: {
    id: string;
    version: string;
    kind: string;
    name?: Record<string, string>;
    config?: Record<string, unknown>;
  }[];
}) {
  const { locale, entityLabel, t } = useI18n();
  const ja = locale === "ja-JP";
  const [target, setTarget] = useState("");
  const origin = node;
  const available = entries.filter((e) =>
    [
      "tool",
      "bundle",
      "skill",
      "memory",
      "source",
      "external_service",
    ].includes(e.kind),
  );
  const labels = disambiguateLabels(
    available,
    (entry) => `${entry.kind}:${entry.id}@${entry.version}`,
    (entry) => entityLabel({ ...entry, name: entry.name ?? {} }),
  );
  const label = (binding: Binding) =>
    (binding.target.registry_node === node
      ? labels.get(
          `${binding.kind}:${binding.target.id}@${binding.target.version}`,
        )
      : undefined) ?? `${t("unavailableEntity")} · ${binding.target.version}`;
  const bindings = value.bindings ?? [];
  const removed = value.remove_default ?? [];
  const requiredDefaults = (selected: Binding[]) => [
    ...(selected.some((binding) => binding.kind === "skill") ||
    hasInstructionalBinding(
      selected,
      entries.map((entry) => ({ ...entry, config: entry.config ?? {} })),
      node,
    )
      ? ["skill_list", "skill_load", "skill_read"]
      : []),
    ...(cluster ? coordinatorDefaults : []),
  ];
  const required = requiredDefaults(bindings);
  const save = (next: BindingConfiguration) =>
    change({
      ...next,
      remove_default: next.remove_default.filter(
        (name) => !requiredDefaults(next.bindings).includes(name),
      ),
    });
  const update = (i: number, patch: Partial<Binding>) =>
    save({
      ...value,
      bindings: bindings.map((b, index) =>
        index === i ? { ...b, ...patch } : b,
      ),
    });
  return (
    <Group legend={ja ? "ツールとデータの追加" : "Added tools and data"}>
      <Hint>
        {ja
          ? "Registryに登録したツール、バンドル、スキル、メモリー、ソースのうち、このエージェントにだけ使わせるものを選びます。すべてのエージェントが最初から持つツールは、下の「組み込みツール」で外せます。メモリーを読むには、メモリーかソースをここで追加してください。実行には現在の権限とProviderが必要です。"
          : "Choose the Registry tools, bundles, skills, memories and sources that this Agent uses in addition to the built-in tools. Remove built-in tools under Built-in tools below. To read memory, add a Memory or Source here. Running still needs current permissions and providers."}
      </Hint>
      <div className={pairClass}>
        <Field label={ja ? "登録Node" : "Registering Node"}>
          <Input
            value={origin}
            readOnly
            className="bg-raised font-mono text-xs text-muted-foreground"
          />
        </Field>
        <Field label={ja ? "追加するもの" : "Item to add"}>
          <NativeSelect
            value={target}
            onChange={(e) => setTarget(e.target.value)}
          >
            <option value="">{ja ? "選択" : "Select"}</option>
            {groupByKind(available, (e) => e.kind).map(([kind, items]) => (
              <optgroup key={kind} label={t(kind)}>
                {items.map((e) => (
                  <option
                    key={`${e.kind}:${e.id}@${e.version}`}
                    value={`${e.kind}:${e.id}@${e.version}`}
                  >
                    {labels.get(`${e.kind}:${e.id}@${e.version}`)}
                  </option>
                ))}
              </optgroup>
            ))}
          </NativeSelect>
        </Field>
      </div>
      <Button
        type="button"
        variant="outline"
        className="justify-self-start"
        disabled={!target || !origin}
        onClick={() => {
          const entry = available.find(
            (e) => `${e.kind}:${e.id}@${e.version}` === target,
          );
          if (
            !entry ||
            bindings.some(
              (b) =>
                b.target.registry_node === origin &&
                b.target.id === entry.id &&
                b.target.version === entry.version,
            )
          )
            return;
          save({
            ...value,
            bindings: [
              ...bindings,
              {
                kind: entry.kind as Binding["kind"],
                target: {
                  registry_node: origin,
                  id: entry.id,
                  version: entry.version,
                },
                narrow: {},
              },
            ],
          });
          setTarget("");
        }}
      >
        {ja ? "このエージェントに追加" : "Add to this Agent"}
      </Button>
      {bindings.length > 0 && (
        <ul className="grid min-w-0 gap-3">
          {bindings.map((binding, i) => (
            <li
              key={`${binding.target.registry_node}:${binding.target.id}@${binding.target.version}`}
              className="grid min-w-0 gap-3 border-l-2 border-brand-line pl-3"
            >
              <div className="flex min-w-0 items-start gap-2">
                <p className="min-w-0 flex-1 text-[13px] font-medium text-foreground [overflow-wrap:anywhere]">
                  {label(binding)}{" "}
                  <span className="font-normal text-muted-foreground">
                    · {t(binding.kind)} ·{" "}
                    <span className="font-mono text-xs">
                      {binding.target.registry_node}
                    </span>
                  </span>
                </p>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={() =>
                    save({
                      ...value,
                      bindings: bindings.filter((_, index) => index !== i),
                    })
                  }
                >
                  {ja ? "このエージェントから外す" : "Remove from this Agent"}
                </Button>
              </div>
              {binding.kind === "tool" && (
                <Field
                  label={
                    ja
                      ? "モデルに提示する名前（省略可）"
                      : "Stable alias (optional)"
                  }
                >
                  <Input
                    className="font-mono text-xs"
                    value={binding.alias ?? ""}
                    pattern="[A-Za-z0-9_-]{1,64}"
                    onChange={(e) =>
                      update(i, { alias: e.target.value || undefined })
                    }
                  />
                </Field>
              )}
              <Field label={ja ? "制限（JSON）" : "Restrictions (JSON)"}>
                <Restrictions
                  value={binding.narrow}
                  change={(narrow) => update(i, { narrow })}
                  ja={ja}
                />
              </Field>
              {binding.kind === "bundle" && (
                <Field
                  label={
                    ja
                      ? "選ぶメンバーID（空欄ならすべて）"
                      : "Member IDs (empty selects all)"
                  }
                >
                  <Input
                    className="font-mono text-xs"
                    value={(binding.members ?? []).join(", ")}
                    onChange={(e) =>
                      update(i, {
                        members: e.target.value
                          .split(",")
                          .map((s) => s.trim())
                          .filter(Boolean),
                      })
                    }
                  />
                </Field>
              )}
            </li>
          ))}
        </ul>
      )}
      <Group nested legend={ja ? "組み込みツール" : "Built-in tools"}>
        <Hint>
          {ja
            ? "すべてのエージェントが最初から持つツールです。チェックを外すとこのエージェントから外れます。workspace_read と human_request は常に含まれ、灰色の項目は今の設定で必要なため外せません。"
            : "Every Agent starts with these tools. Clear one to remove it from this Agent. workspace_read and human_request are always included. Grayed-out tools are required by the current setup."}
        </Hint>
        <div className="grid min-w-0 gap-x-4 gap-y-1.5 sm:grid-cols-2">
          {defaults.map((name) => (
            <Check
              key={name}
              className="font-mono text-xs"
              disabled={required.includes(name)}
              checked={!removed.includes(name)}
              onChange={(e) =>
                save({
                  ...value,
                  remove_default: e.target.checked
                    ? removed.filter((n) => n !== name)
                    : [...removed, name],
                })
              }
            >
              {name}
            </Check>
          ))}
        </div>
      </Group>
    </Group>
  );
}

export function nativeMemoryProvider(
  config: Record<string, unknown> | undefined,
  entries: {
    id: string;
    version: string;
    kind: string;
    config: Record<string, unknown>;
  }[],
  requireReads = false,
): { id: string; version: string } | undefined {
  const bindings = (config?.bindings ?? []) as Binding[];
  const removed = (config?.remove_default ?? []) as string[];
  const reads = ["memory_recall", "memory_reflect"];
  if (
    requireReads &&
    reads.every((operation) => removed.includes(operation)) &&
    !bindings.some(
      (binding) =>
        binding.kind === "tool" &&
        entries.some(
          (entry) =>
            entry.id === binding.target.id &&
            entry.version === binding.target.version &&
            reads.includes(String(entry.config.operation)),
        ),
    )
  )
    return undefined;
  const provider = bindings.find(
    (binding) =>
      binding.kind === "memory" &&
      entries.some(
        (entry) =>
          entry.kind === "memory" &&
          entry.id === binding.target.id &&
          entry.version === binding.target.version &&
          entry.config.engine === "hindsight_rust",
      ),
  );
  return provider
    ? { id: provider.target.id, version: provider.target.version }
    : undefined;
}
