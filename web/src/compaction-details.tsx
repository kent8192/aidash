import { Disclosure, Facts, Hint, Notice } from "./components/patterns";
import { DeciderRecipients } from "./catalog-approval";
import { deciderRecipients } from "./decider";
import { useI18n } from "./ui";

type PinnedBinding = {
  identity: { id: string; version: string; registry_deployment?: string };
  definition: { kind: string; config: unknown };
  digest: string;
  excluded_reason: string | null;
};
/** Read the Run's admitted snapshot, never a newer mutable Registry choice. */
export function RunCompactionDetails({
  bindings,
  reason,
}: {
  bindings: PinnedBinding[];
  reason?: string | null;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const compaction = bindings.filter(
    (binding) =>
      binding.definition.kind === "decider" &&
      (binding.definition.config as { hook?: string } | null)?.hook ===
        "compaction",
  );
  const pauses: Record<string, [string, string]> = {
    decider_unapproved: [
      "The pinned compaction Decider needs Catalog Approval.",
      "固定された圧縮 Decider に Catalog Approval が必要です。",
    ],
    decider_calls_exhausted: [
      "The compaction call budget is exhausted.",
      "圧縮の呼び出し予算を使い切りました。",
    ],
    decider_unavailable: [
      "The compaction Decider is unavailable.",
      "圧縮 Decider が利用できません。",
    ],
  };
  return (
    <>
      {reason && pauses[reason] && (
        <Notice tone="warning" role="status">
          {pauses[reason][ja ? 1 : 0]} <code>{reason}</code>
        </Notice>
      )}
      {compaction.length > 0 && (
        <Disclosure
          summary={
            ja ? "Run が固定した圧縮 Decider" : "Run-pinned compaction Decider"
          }
        >
          {compaction.map((binding) => {
            const recipients = deciderRecipients(binding.definition);
            return (
              <div
                key={`${binding.identity.id}@${binding.identity.version}`}
                className="grid min-w-0 gap-2"
              >
                <Facts
                  items={[
                    [
                      "Decider",
                      `${binding.identity.id}@${binding.identity.version}`,
                    ],
                    [
                      ja ? "定義ダイジェスト" : "Definition digest",
                      binding.digest,
                    ],
                    [
                      ja ? "採用状態" : "Admission",
                      binding.excluded_reason ?? (ja ? "固定済み" : "Pinned"),
                    ],
                  ]}
                />
                {recipients && <DeciderRecipients recipients={recipients} />}
              </div>
            );
          })}
          <Hint>
            {ja
              ? "固定された定義の実行にも、現在の Catalog Approval・認証情報・呼び出し予算が必要です。"
              : "Executing the pinned definition still requires current Catalog Approval, credentials and call budget."}
          </Hint>
        </Disclosure>
      )}
    </>
  );
}

/** Registration/approval reads the trusted graph rather than recomputing an
 * implicit dependency from the authored configuration. */
export function RegisteredCompactionDetails({
  normalization,
  definitions,
}: {
  normalization?: {
    bindings: {
      binding: { kind: string; target: { id: string; version: string } };
      origin: string;
    }[];
  } | null;
  definitions: { id: string; version: string; kind: string; config: unknown }[];
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  if (!normalization) return null;
  const compaction = normalization.bindings
    .filter(({ binding }) => binding.kind === "decider")
    .flatMap(({ binding, origin }) => {
      const definition = definitions.find(
        (definition) =>
          definition.id === binding.target.id &&
          definition.version === binding.target.version &&
          definition.kind === "decider" &&
          (definition.config as { hook?: string } | null)?.hook ===
            "compaction",
      );
      return definition || binding.target.id === "aidash.compaction-decider"
        ? [{ reference: binding.target, origin, definition }]
        : [];
    });
  return (
    <Disclosure
      summary={
        ja ? "登録時に固定された圧縮" : "Registered compaction dependency"
      }
    >
      {compaction.length ? (
        compaction.map(({ reference, origin, definition }) => {
          const recipients = definition ? deciderRecipients(definition) : null;
          return (
            <div
              key={`${reference.id}@${reference.version}`}
              className="grid min-w-0 gap-2"
            >
              <Facts
                items={[
                  ["Decider", `${reference.id}@${reference.version}`],
                  [ja ? "バインディングの由来" : "Binding origin", origin],
                ]}
              />
              {recipients && <DeciderRecipients recipients={recipients} />}
            </div>
          );
        })
      ) : (
        <Hint>
          {ja
            ? "登録されたグラフに圧縮 Decider はありません。"
            : "The registered dependency graph contains no compaction Decider."}
        </Hint>
      )}
      <Hint>
        {ja
          ? "この正確なバージョンの Catalog Approval が必要です。既定の追加や登録だけでは実行権限になりません。"
          : "Catalog Approval is required for this exact version. Default binding and registration do not authorize execution."}
      </Hint>
    </Disclosure>
  );
}
