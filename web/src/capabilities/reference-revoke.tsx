import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Alert, Hint } from "../components/patterns";
import { Button } from "../components/ui/button";
import { ApiError, apiFetch } from "../transport";
import { useI18n } from "../ui";
import { post } from "./client";

/** The Reference files a `reference_attachments` Source binds, if it is one. */
export function boundReferences(
  config: Record<string, unknown> | undefined,
): { reference_id: string; path: string }[] {
  const source = config?.source as
    | { adapter?: string; references?: unknown }
    | undefined;
  if (source?.adapter !== "reference_attachments") return [];
  if (!Array.isArray(source.references)) return [];
  return source.references.filter(
    (value): value is { reference_id: string; path: string } =>
      typeof value?.reference_id === "string" &&
      typeof value?.path === "string",
  );
}

type Revocation = { reference_id: string; revision: number; state: string };

function BoundReference({ id, path }: { id: string; path: string }) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const key = ["reference-revocation", id];
  // The route answers 404 to a subject whose policy lacks `reference.delete`,
  // and for a file already revoked. Either way there is nothing to offer, so a
  // 404 resolves to `null`: a failed refetch would otherwise keep the old data
  // and show the Revoke button again. Other failures stay errors.
  const revocation = useQuery({
    queryKey: key,
    queryFn: async () => {
      try {
        return await apiFetch<Revocation>(`/api/references/${id}/revocation`);
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) return null;
        throw e;
      }
    },
    retry: false,
  });
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const revoke = async (revision: number) => {
    setBusy(true);
    setError("");
    try {
      await post(`/references/${id}/revoke`, { expected_revision: revision });
      setConfirming(false);
      // A revoked file has nothing left to offer; do not wait for the refetch.
      client.setQueryData(key, null);
    } catch (e) {
      setError(String(e));
    } finally {
      // The revision may have moved, or the file may be gone: ask again.
      await client.invalidateQueries({ queryKey: key });
      setBusy(false);
    }
  };
  const current = revocation.data;
  return (
    <li className="grid min-w-0 gap-1 px-3 py-2">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <span className="min-w-0 flex-1 font-mono text-xs [overflow-wrap:anywhere]">
          {path}
        </span>
        {current &&
          (confirming ? (
            <>
              <Button
                type="button"
                size="sm"
                variant="destructive"
                disabled={busy}
                onClick={() => void revoke(current.revision)}
              >
                {ja ? "取り消しを確定" : "Confirm revoke"}
              </Button>
              <Button
                type="button"
                size="sm"
                variant="outline"
                disabled={busy}
                onClick={() => setConfirming(false)}
              >
                {ja ? "やめる" : "Cancel"}
              </Button>
            </>
          ) : (
            <Button
              type="button"
              size="sm"
              variant="outline"
              onClick={() => setConfirming(true)}
            >
              {ja ? "取り消す" : "Revoke"}
            </Button>
          ))}
        {revocation.data === null && (
          <Hint>
            {ja
              ? "取り消せません（権限がないか、すでに取り消されています）。"
              : "Cannot revoke (not permitted, or already revoked)."}
          </Hint>
        )}
      </div>
      {revocation.isError && <Alert>{String(revocation.error)}</Alert>}
      {error && <Alert>{error}</Alert>}
    </li>
  );
}

/** Lets a person whose policy grants `reference.delete` stop a bound file. */
export function ReferenceRevocations({
  references,
}: {
  references: { reference_id: string; path: string }[];
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  if (references.length === 0) return null;
  return (
    <section className="grid min-w-0 gap-2 border-t border-border pt-4">
      <h4 className="text-[13px] font-semibold text-foreground">
        {ja ? "参照ファイル" : "Reference files"}
      </h4>
      <Hint>
        {ja
          ? "取り消すと、新しい Run はこのファイルを使えず、コピーを持つ作業領域も使えなくなります。"
          : "Revoking refuses new Runs that bind the file and makes any Working Area holding a copy unusable."}
      </Hint>
      <ul className="divide-y divide-border rounded-md border border-border">
        {references.map(({ reference_id, path }) => (
          <BoundReference key={reference_id} id={reference_id} path={path} />
        ))}
      </ul>
    </section>
  );
}
