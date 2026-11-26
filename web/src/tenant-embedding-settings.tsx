import { useState } from "react";
import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Alert, Hint, Loading } from "./components/patterns";
import { Button } from "./components/ui/button";
import { NativeSelect } from "./components/ui/native-select";
import { apiFetch } from "./transport";
import { Field, Panel, useI18n } from "./ui";

type EmbeddingRef = { id: string; version: string };
type SettingsResponse = {
  settings: {
    active: EmbeddingRef;
    pending: EmbeddingRef | null;
    revision: number;
  };
  embeddings: EmbeddingRef[];
  estimate: {
    thread_indexes: number;
    thread_entries: number;
    drive_files: number;
  } | null;
};

function versionKey(reference: EmbeddingRef) {
  return `${reference.id}@${reference.version}`;
}

export function TenantEmbeddingSettings({ tenant }: { tenant: string }) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const path = `/api/tenants/${encodeURIComponent(tenant)}/embedding-settings`;
  const key = ["tenant-embedding-settings", tenant];
  const [selection, setSelection] = useState("");
  const query = useQuery({
    queryKey: key,
    queryFn: () => apiFetch<SettingsResponse>(path),
    retry: false,
  });
  const change = useMutation({
    mutationFn: ({
      embedding,
      revision,
      confirm,
    }: {
      embedding: EmbeddingRef;
      revision: number;
      confirm: boolean;
    }) =>
      apiFetch<SettingsResponse>(path, {
        method: "PUT",
        body: JSON.stringify({
          embedding,
          expected_revision: revision,
          confirm,
        }),
      }),
    onSuccess: (data) => client.setQueryData(key, data),
  });
  const data = query.data;
  const selected = data?.embeddings.find(
    (ref) => versionKey(ref) === selection,
  );
  const estimate = change.data?.estimate;
  return (
    <Panel title={ja ? "埋め込みモデル" : "Embedding model"}>
      {query.isPending && <Loading />}
      {query.isError && (
        <Alert retry={() => void query.refetch()}>{query.error.message}</Alert>
      )}
      {change.isError && <Alert>{change.error.message}</Alert>}
      {data && (
        <div className="grid gap-3">
          <Hint>
            {ja ? "使用中のモデル" : "Active model"}{" "}
            <code>{versionKey(data.settings.active)}</code>
          </Hint>
          {data.settings.pending && (
            <Hint role="status">
              {ja ? "切り替え待ち" : "Pending switch"}{" "}
              <code>{versionKey(data.settings.pending)}</code>
            </Hint>
          )}
          <Field label={ja ? "切り替え先" : "Replacement model"}>
            <NativeSelect
              value={selection}
              disabled={change.isPending}
              onChange={(event) => {
                setSelection(event.target.value);
                change.reset();
              }}
            >
              <option value="">{ja ? "選択" : "Choose a model"}</option>
              {data.embeddings.map((ref) => (
                <option key={versionKey(ref)} value={versionKey(ref)}>
                  {versionKey(ref)}
                </option>
              ))}
            </NativeSelect>
          </Field>
          {estimate && (
            <Hint role="status">
              {ja
                ? `${estimate.thread_indexes} 件の Thread インデックスに ${estimate.thread_entries} 件の項目があります。Drive の対象は ${estimate.drive_files} ファイルです。`
                : `${estimate.thread_entries} entries across ${estimate.thread_indexes} Thread indexes, plus ${estimate.drive_files} Drive files.`}
            </Hint>
          )}
          <div className="flex justify-end">
            <Button
              disabled={
                !selected ||
                change.isPending ||
                versionKey(selected) === versionKey(data.settings.active)
              }
              onClick={() => {
                if (selected)
                  change.mutate({
                    embedding: selected,
                    revision: data.settings.revision,
                    confirm: Boolean(estimate),
                  });
              }}
            >
              {estimate
                ? ja
                  ? "切り替えを確定"
                  : "Confirm switch"
                : ja
                  ? "再埋め込みを見積もる"
                  : "Estimate re-embedding"}
            </Button>
          </div>
        </div>
      )}
    </Panel>
  );
}
