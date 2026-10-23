import { useState } from "react";
import { Check, Group, Hint, pairClass } from "./components/patterns";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Field, useI18n } from "./ui";

type Entry = {
  id: string;
  version: string;
  kind: string;
  name?: Record<string, string>;
  config?: Record<string, unknown>;
};
export function ContextSourceFields({
  entries,
  node,
  adapter,
}: {
  entries: Entry[];
  node: string;
  adapter: string;
}) {
  const { locale, entityLabel } = useI18n();
  const ja = locale === "ja-JP";
  const [enabled, setEnabled] = useState(false);
  const [pin, setPin] = useState("always");
  const [required, setRequired] = useState(false);
  const [mode, setMode] = useState("shadow");
  const operation =
    adapter === "native_memory"
      ? "semantic_memory"
      : ["private_references", "reference_attachments"].includes(adapter)
        ? "references"
        : ["skill_attachments", "skill_roots"].includes(adapter)
          ? "skills"
          : adapter;
  const ref = (entry: Entry) =>
    JSON.stringify({
      registry_deployment: node,
      id: entry.id,
      version: entry.version,
    });
  const options = (kind: string) =>
    entries
      .filter(
        (entry) =>
          entry.kind === kind &&
          (kind !== "decider" ||
            (entry.config?.mode === mode &&
              entry.config?.hook === "context_selection")),
      )
      .map((entry) => (
        <option key={ref(entry)} value={ref(entry)}>
          {entityLabel({ ...entry, name: entry.name ?? {} })} · {entry.id}@
          {entry.version}
        </option>
      ));
  return (
    <Group legend={ja ? "ソースの取得ポリシー" : "Source retrieval policy"}>
      <Check
        name="source_declaration"
        checked={enabled}
        onChange={(event) => setEnabled(event.target.checked)}
      >
        {ja
          ? "明示的なソース宣言を設定"
          : "Configure an explicit source declaration"}
      </Check>
      <Hint>
        {ja
          ? "未設定なら現在の取得と表示を維持します。宣言だけでは権限は付与されず、実行時に必須となる Home の制約も解除できません。"
          : "Leaving this unset preserves current retrieval and rendering. A declaration grants no authority and cannot relax runtime-required Home obligations."}
      </Hint>
      <Hint>
        {ja
          ? "Agent の共有 native クエリーでは、プライマリーメモリー・各 bank Source・workspace の取得のクエリーポリシーを揃えてください。省略した定義も task-inputs@1 として扱われます。accepted-inputs-first@1 を使う場合は、すべての有効な構成要素で明示的に選択してください。"
          : "The Agent's shared native query requires matching query policies across primary memory, every bank Source and workspace retrieval. An omitted policy means task-inputs@1. To use accepted-inputs-first@1, explicitly select it on every effective contributor."}
      </Hint>
      {enabled && (
        <>
          <div className={pairClass}>
            <Field
              label={
                ja
                  ? "プロバイダー操作のプロバイダー"
                  : "Provider operation: provider"
              }
            >
              <Input
                name="source_provider"
                required
                maxLength={128}
                defaultValue="aidash.native-context/1"
              />
            </Field>
            <Field label={ja ? "プロバイダー操作" : "Provider operation"}>
              <Input
                name="source_operation"
                required
                maxLength={128}
                defaultValue={operation}
              />
            </Field>
          </div>
          <Field label={ja ? "取得方式" : "Pin policy"}>
            <NativeSelect
              name="source_pin"
              value={pin}
              onChange={(event) => setPin(event.target.value)}
            >
              <option value="always">Always</option>
              <option value="on_demand" disabled={required}>
                OnDemand
              </option>
              <option value="scored">Scored</option>
            </NativeSelect>
          </Field>
          <Hint>
            {ja
              ? "Always は境界ごとの上限付き寄与です。OnDemand は宣言した読み取りツールから取得します。Scored は候補ごとの Noul 判断を使います。"
              : "Always contributes bounded context at each boundary. OnDemand uses declared read tools. Scored asks candidate-bound Noul keep questions."}
          </Hint>
          <Check
            name="source_required_home"
            checked={required}
            onChange={(event) => {
              setRequired(event.target.checked);
              if (event.target.checked) {
                setMode("shadow");
                if (pin === "on_demand") setPin("always");
              }
            }}
          >
            {ja ? "必須の Home 寄与" : "Required-Home contribution"}
          </Check>
          {required && (
            <Hint>
              {ja
                ? "必須 Home は Always と受領証を維持します。Shadow で観測できますが、Scored Enforce では除外できません。"
                : "Required-Home retains Always delivery and mandatory receipts. Shadow may observe it; Scored Enforce cannot remove it."}
            </Hint>
          )}
          {pin === "scored" && (
            <>
              <Field label={ja ? "選択の動作" : "Selection mode"}>
                <NativeSelect
                  name="source_mode"
                  value={mode}
                  onChange={(event) => setMode(event.target.value)}
                >
                  <option value="shadow">Shadow</option>
                  <option value="enforce" disabled={required}>
                    Enforce
                  </option>
                </NativeSelect>
              </Field>
              <Field
                label={ja ? "正確な選択 Decider" : "Exact selection Decider"}
              >
                <NativeSelect name="source_decider" required defaultValue="">
                  <option value="">{ja ? "選択" : "Select"}</option>
                  {options("decider")}
                </NativeSelect>
              </Field>
              <Hint>
                {ja
                  ? "正確なバージョンの Catalog Approval と開示権限が必要です。Shadow はリクエストを変えません。Enforce は未承認・利用不可・予算超過・開示不可のとき理由付きで一時停止します。Always へ切り替わりません。"
                  : "The exact version needs Catalog Approval and disclosure authority. Shadow preserves the request. Enforce pauses on unapproved, unavailable, exhausted or forbidden disclosure outcomes; it never falls back to Always."}
              </Hint>
              <Check name="source_disclose_previews" required>
                {ja
                  ? "認可済み候補の上限付きプレビューの開示を宣言"
                  : "Declare disclosure of bounded authorized candidate previews"}
              </Check>
              <Check name="source_disclose_query" required>
                {ja
                  ? "宣言したクエリー素材の開示を宣言"
                  : "Declare disclosure of the query material"}
              </Check>
            </>
          )}
          <div className={pairClass}>
            <Field label={ja ? "変動の単位" : "Volatility"}>
              <NativeSelect name="source_volatility" defaultValue="step">
                <option value="step">Step</option>
                <option value="run">Run</option>
              </NativeSelect>
            </Field>
            <Field label={ja ? "クエリーポリシー" : "Query policy"}>
              <NativeSelect name="source_query" defaultValue="task-inputs@1">
                <option value="task-inputs@1">task-inputs@1</option>
                <option value="accepted-inputs-first@1">
                  accepted-inputs-first@1
                </option>
              </NativeSelect>
            </Field>
          </div>
          <Hint>
            {ja
              ? "accepted-inputs-first@1 は認可済みの受理入力を新しい順に並べ、Task の初期入力を続けます。要約は加えません。更新は revision-bound@1、クエリーと入力はダイジェストで固定します。"
              : "accepted-inputs-first@1 orders authorized accepted inputs newest first, followed by the Task seed, without summaries. Refresh uses revision-bound@1 with exact input and query digests."}
          </Hint>
          {(["read", "write"] as const).map((operation) => (
            <Field
              key={operation}
              label={
                operation === "read"
                  ? ja
                    ? "正確な読み取りツール"
                    : "Exact read tools"
                  : ja
                    ? "正確な書き込みツール"
                    : "Exact write tools"
              }
            >
              <NativeSelect
                name={`source_${operation}`}
                multiple
                size={4}
                className="h-auto min-h-24"
              >
                {options("tool")}
              </NativeSelect>
            </Field>
          ))}
          <Field
            label={
              ja ? "省略部分を読む正確なツール" : "Exact omitted-detail tool"
            }
          >
            <NativeSelect name="source_detail" defaultValue="">
              <option value="">{ja ? "なし" : "None"}</option>
              {options("tool")}
            </NativeSelect>
          </Field>
          <Hint>
            {ja
              ? "ツールのスキーマや権限をコピーせず、既存のバージョンを参照します。取得・選択・最終整形・受理された推論への配信は別の段階で、採点だけでは全文の既読証明になりません。"
              : "These reference existing Tool versions and their authority. Retrieval, selection, final fitting and accepted delivery are separate stages; scoring alone is no proof of a full read."}
          </Hint>
        </>
      )}
    </Group>
  );
}
