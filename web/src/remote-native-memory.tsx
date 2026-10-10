import { nativeMemoryProvider } from "./agent-bindings";
import { useState } from "react";
import { useInfiniteQuery } from "@tanstack/react-query";
import type { Entry, EntityRef } from "./types";
import { apiFetch } from "./transport";
import { Button } from "./components/ui/button";
import { NativeSelect } from "./components/ui/native-select";
import { Field, useI18n } from "./ui";
import { Alert, Check, Group, Hint, Notice } from "./components/patterns";

type Participant = {
  id: string;
  agent: EntityRef;
  revision: number;
};
type NativeRequest = {
  participant: string;
  expected_revision: number;
  provider: EntityRef;
};

export function nativeMemoryRequest(
  fields: FormData,
): NativeRequest | undefined {
  const value = fields.get("native_memory");
  return typeof value === "string" && value ? JSON.parse(value) : undefined;
}

export function HomeNativeMemoryFields({
  workspace,
  entries,
  required = false,
  generated = false,
}: {
  workspace: string;
  entries: Entry[];
  required?: boolean;
  generated?: boolean;
}) {
  const { locale, t, local } = useI18n();
  const ja = locale === "ja-JP";
  const [optional, setOptional] = useState(false);
  const [selected, setSelected] = useState("");
  const enabled = required || optional;
  const query = useInfiniteQuery({
    queryKey: ["memory", workspace, "participants"],
    initialPageParam: "",
    queryFn: ({ pageParam }) =>
      apiFetch<{ items: Participant[]; next: string | null }>(
        `/api/workspaces/${workspace}/memory/participants${pageParam ? `?after=${encodeURIComponent(pageParam)}` : ""}`,
      ),
    getNextPageParam: (page) => page.next || undefined,
    enabled,
    retry: false,
  });
  const choices = query.isError
    ? []
    : (query.data?.pages.flatMap((page) => page.items) ?? []).flatMap(
        (participant) => {
          const agent = entries.find(
            (entry) =>
              entry.kind === "agent" &&
              entry.id === participant.agent.id &&
              entry.version === participant.agent.version,
          );
          const provider = nativeMemoryProvider(agent?.config, entries, true);
          return agent &&
            provider &&
            entries.some(
              (entry) =>
                entry.kind === "memory" &&
                entry.id === provider.id &&
                entry.version === provider.version,
            )
            ? [
                {
                  participant,
                  agent,
                  request: {
                    participant: participant.id,
                    expected_revision: participant.revision,
                    provider,
                  },
                },
              ]
            : [];
        },
      );
  const chosen = choices.find((choice) => choice.participant.id === selected);
  return (
    <Group
      nested
      legend={ja ? "Homeの論理Agentの記憶" : "Home logical Agent memory"}
    >
      <Check
        checked={enabled}
        disabled={required}
        onChange={(event) => setOptional(event.target.checked)}
      >
        {ja
          ? "Homeの私有・共有記憶を参照する"
          : "Use private and shared memory at Home"}
      </Check>
      {enabled && (
        <>
          <Field
            label={
              generated
                ? ja
                  ? "Homeの記憶設定テンプレート"
                  : "Home memory configuration template"
                : ja
                  ? "Homeの論理Agent"
                  : "Home logical Agent"
            }
          >
            <NativeSelect
              required
              value={chosen?.participant.id ?? ""}
              onChange={(event) => setSelected(event.target.value)}
            >
              <option value="">{t("choose")}</option>
              {choices.map((choice) => (
                <option
                  key={choice.participant.id}
                  value={choice.participant.id}
                >
                  {local(choice.agent.name)} · {choice.participant.id} · r
                  {choice.participant.revision}
                </option>
              ))}
            </NativeSelect>
          </Field>
          <input
            type="hidden"
            name="native_memory"
            value={chosen ? JSON.stringify(chosen.request) : ""}
          />
          {query.isError && (
            <Alert>{query.error.message}</Alert>
          )}
          {!query.isPending && !choices.length && (
            <Notice tone="warning" role="status">
              {ja
                ? "記憶を有効にした論理Agentを、このWorkspaceの記憶画面で作成してください。"
                : "Create a logical Agent with memory enabled in this Workspace's memory view."}
            </Notice>
          )}
          {query.hasNextPage && (
            <Button
              variant="ghost"
              size="sm"
              className="justify-self-start"
              type="button"
              disabled={query.isFetchingNextPage}
              onClick={() => void query.fetchNextPage()}
            >
              {ja ? "論理Agentを追加で表示" : "Load more logical Agents"}
            </Button>
          )}
          <Hint>
            {generated
              ? ja
                ? "Homeに新しい論理Agent IDを発行し、私有記憶を空で開始します。テンプレートの本文はコピーせず、設定された共有Sourceを参照します。"
                : "Home issues a fresh logical Agent ID with empty private memory. It uses the configured shared Sources without copying template bodies."
              : ja
                ? "選択したHomeの記憶を使用します。実行Nodeの記憶は含めません。"
                : "Execution uses the selected Home memory. Executor-local memory is excluded."}
          </Hint>
        </>
      )}
    </Group>
  );
}
