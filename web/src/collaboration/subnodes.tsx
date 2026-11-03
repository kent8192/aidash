import { useState } from "react";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { Button } from "../components/ui/button";
import { Alert, Facts } from "../components/patterns";
import { runSubnodes, subnodeDissolve } from "../generated/aidash";
import type { EntityRef, SubnodeView } from "../generated/models";
import { subnodeActive, subnodeTitle } from "../subnodes-model";
import { ApiError } from "../transport";
import { Badge, useI18n } from "../ui";

/** The Subnodes a Run created (#660), shown under that Run. Nothing renders
 *  while the Run has none. Whoever may cancel the Run may dissolve them. */
export function RunSubnodes({
  run,
  node,
  agentLabel,
}: {
  run: string;
  node: string;
  agentLabel: (node: string, reference: EntityRef) => string;
}) {
  const { t } = useI18n();
  const client = useQueryClient();
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  // A 403 on dissolve means this person cannot cancel the Run either.
  const [forbidden, setForbidden] = useState(false);
  const key = ["run-subnodes", node, run];
  const query = useQuery({
    queryKey: key,
    queryFn: () => runSubnodes(run),
    refetchInterval: 2000,
    retry: false,
  });
  const subnodes = query.isError ? [] : (query.data ?? []);
  if (subnodes.length === 0) return null;
  const dissolve = (subnode: SubnodeView) => {
    if (busy || !window.confirm(t("subnodeConfirmDissolve"))) return;
    setBusy(subnode.id);
    setError("");
    void subnodeDissolve(subnode.id)
      .then(() => client.invalidateQueries({ queryKey: key }))
      .catch((cause) => {
        if (cause instanceof ApiError && cause.status === 403)
          setForbidden(true);
        setError(cause instanceof Error ? cause.message : String(cause));
      })
      .finally(() => setBusy(""));
  };
  return (
    <section
      aria-label={t("subnodes")}
      className="grid min-w-0 gap-3 border-t border-border pt-4"
    >
      <h3 className="text-[13px] font-semibold text-foreground">
        {t("subnodes")}
      </h3>
      {error && <Alert>{error}</Alert>}
      <ul className="grid min-w-0 gap-4">
        {subnodes.map((subnode) => {
          const active = subnodeActive(subnode);
          return (
            <li
              key={subnode.id}
              className="grid min-w-0 gap-2 border border-border p-3"
            >
              <div className="flex min-w-0 flex-wrap items-center gap-2">
                <span className="min-w-0 text-[13px] font-medium text-foreground [overflow-wrap:anywhere]">
                  {subnodeTitle(subnode)}
                </span>
                <Badge
                  value={active ? "subnodeActive" : "subnodeDissolved"}
                  tone={active ? "brand" : "neutral"}
                />
                {active && !forbidden && (
                  <Button
                    variant="destructive"
                    type="button"
                    className="ml-auto"
                    disabled={busy !== ""}
                    onClick={() => dissolve(subnode)}
                  >
                    {t("subnodeDissolve")}
                  </Button>
                )}
              </div>
              <Facts
                items={[
                  [t("subnodeLeader"), agentLabel(node, subnode.leader)],
                  [
                    t("subnodeMembers"),
                    subnode.members
                      .map((member) =>
                        agentLabel(node, {
                          id: member.agent_id,
                          version: member.agent_version,
                        }),
                      )
                      .join(", ") || t("subnodeNone"),
                  ],
                  [
                    t("subnodeAllowedTools"),
                    subnode.allowance.tools.join(", ") || t("subnodeNone"),
                  ],
                  [
                    t("subnodeAllowedSecrets"),
                    subnode.allowance.secrets.join(", ") || t("subnodeNone"),
                  ],
                  ...(subnode.ended_reason
                    ? [[t("subnodeEndedReason"), subnode.ended_reason] as const]
                    : []),
                ]}
              />
              {subnode.member_runs.length > 0 && (
                <ul className="grid min-w-0 divide-y divide-border border-y border-border">
                  {subnode.member_runs.map((member) => (
                    <li
                      key={member.id}
                      className="flex min-w-0 flex-wrap items-center gap-2 py-1.5 text-[13px] text-foreground"
                    >
                      <span className="min-w-0 truncate">
                        {agentLabel(node, member.agent)}
                      </span>
                      <Badge value={member.state} />
                      <Badge value={member.control} />
                    </li>
                  ))}
                </ul>
              )}
            </li>
          );
        })}
      </ul>
    </section>
  );
}
