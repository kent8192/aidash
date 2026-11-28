import { useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Alert, Hint } from "../components/patterns";
import { Button } from "../components/ui/button";
import { referenceRevoke } from "../generated/aidash";
import type { ReferenceRevocationHint } from "../generated/models";
import { useI18n } from "../ui";

function BoundReference({
  reference_id: id,
  path,
  may_revoke,
  revision,
}: ReferenceRevocationHint) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const client = useQueryClient();
  const [confirming, setConfirming] = useState(false);
  const [busy, setBusy] = useState(false);
  const [revoked, setRevoked] = useState(false);
  const [error, setError] = useState("");
  const revoke = async (revision: number) => {
    setBusy(true);
    setError("");
    try {
      await referenceRevoke(id, { expected_revision: revision });
      setConfirming(false);
      setRevoked(true);
    } catch (e) {
      setError(String(e));
    } finally {
      // The revision may have moved, or the file may be gone: ask again.
      await Promise.all([
        client.invalidateQueries({
          queryKey: ["source-reference-revocations"],
        }),
        client.invalidateQueries({ queryKey: ["registry-versions"] }),
      ]);
      setBusy(false);
    }
  };
  const canRevoke = may_revoke && revision != null && !revoked;
  return (
    <li className="grid min-w-0 gap-1 px-3 py-2">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <span className="min-w-0 flex-1 font-mono text-xs [overflow-wrap:anywhere]">
          {path}
        </span>
        {canRevoke &&
          (confirming ? (
            <>
              <Button
                type="button"
                size="sm"
                variant="destructive"
                disabled={busy}
                onClick={() => void revoke(revision)}
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
        {!canRevoke && (
          <Hint>
            {ja
              ? "取り消せません（権限がないか、すでに取り消されています）。"
              : "Cannot revoke (not permitted, or already revoked)."}
          </Hint>
        )}
      </div>
      {error && <Alert>{error}</Alert>}
    </li>
  );
}

/** Lets a person whose policy grants `reference.delete` stop a bound file. */
export function ReferenceRevocations({
  references = [],
}: {
  references?: ReferenceRevocationHint[];
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
          ? "取り消すと、Run はこのファイルを読めなくなります。"
          : "Revoking removes Run access to this Reference file."}
      </Hint>
      <ul className="divide-y divide-border rounded-md border border-border">
        {references.map((reference) => (
          <BoundReference key={reference.reference_id} {...reference} />
        ))}
      </ul>
    </section>
  );
}
