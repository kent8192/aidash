import { Field, JsonView, useI18n } from "./ui";
import { Hint, Disclosure } from "./components/patterns";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";

export type ProviderToolParameters = {
  max_tool_calls: number;
  parameters: Record<string, unknown>;
};

export function providerTool(id: string | undefined): string | undefined {
  const name = id
    ?.replace(/@1$/, "")
    .replace(/^aidash\.bundle\.provider_/, "provider.openrouter.");
  return ["web_search", "web_fetch", "datetime", "fusion"].find(
    (tool) => name === `provider.openrouter.${tool}`,
  );
}

export function providerToolDefaults(tool: string): ProviderToolParameters {
  const parameters: Record<string, unknown> =
    tool === "web_search"
      ? { engine: "exa", max_results: 5, max_total_results: 15 }
      : tool === "web_fetch"
        ? { engine: "openrouter", max_content_tokens: 8192 }
        : tool === "datetime"
          ? { timezone: "UTC" }
          : {
              analysis_models: [],
              model: "",
              max_tool_calls: 2,
              max_completion_tokens: 4096,
            };
  return { max_tool_calls: 3, parameters };
}

export function ProviderToolFields({
  tool,
  value,
  change,
}: {
  tool: string;
  value: ProviderToolParameters;
  change: (value: ProviderToolParameters) => void;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const parameters =
    value.parameters &&
    typeof value.parameters === "object" &&
    !Array.isArray(value.parameters)
      ? value.parameters
      : {};
  const set = (key: string, next: unknown) =>
    change({ ...value, parameters: { ...parameters, [key]: next } });
  const count = (key: string, maximum: number, label: string) => (
    <Field label={`${label} (1–${maximum})`} key={key}>
      <Input
        type="number"
        required
        min={1}
        max={maximum}
        step={1}
        value={Number(parameters[key] ?? 1)}
        onChange={(event) => set(key, Number(event.target.value))}
      />
    </Field>
  );
  return (
    <div className="grid gap-3">
      <Hint>
        {ja
          ? "Provider tool は OpenRouter が実行します。Catalog Approval のみで利用でき、結果は Research の証拠にはなりません。この設定は Run の Binding に固定され、モデルは変更できません。"
          : "OpenRouter executes this Provider tool with Catalog Approval as its only consent. Its results are not Research evidence. These parameters are pinned in the Run Binding and cannot be changed by the model."}
      </Hint>
      <Field
        label={
          ja
            ? "リクエスト全体の最大ツール回数 (1–8)"
            : "Maximum tool calls per request (1–8)"
        }
      >
        <Input
          type="number"
          required
          min={1}
          max={8}
          step={1}
          value={value.max_tool_calls}
          onChange={(event) =>
            change({ ...value, max_tool_calls: Number(event.target.value) })
          }
        />
      </Field>
      {["web_search", "web_fetch"].includes(tool) && (
        <Field label={ja ? "取得サービス" : "Retrieval engine"}>
          <NativeSelect
            value={String(parameters.engine)}
            onChange={(event) => set("engine", event.target.value)}
          >
            {(tool === "web_search"
              ? ["exa", "parallel", "perplexity"]
              : ["openrouter", "exa", "parallel"]
            ).map((engine) => (
              <option key={engine}>{engine}</option>
            ))}
          </NativeSelect>
        </Field>
      )}
      {tool === "web_search" && (
        <>
          {count(
            "max_results",
            10,
            ja ? "検索1回の最大結果数" : "Maximum results per search",
          )}
          {count(
            "max_total_results",
            30,
            ja ? "累計の最大結果数" : "Maximum total results",
          )}
        </>
      )}
      {tool === "web_fetch" &&
        count(
          "max_content_tokens",
          16384,
          ja ? "取得内容の最大トークン数" : "Maximum content tokens",
        )}
      {tool === "datetime" && (
        <Field label={ja ? "タイムゾーン" : "Timezone"}>
          <Input
            required
            value={String(parameters.timezone ?? "UTC")}
            onChange={(event) => set("timezone", event.target.value)}
          />
        </Field>
      )}
      {tool === "fusion" && (
        <>
          <Field
            label={
              ja
                ? "分析モデル（カンマ区切り、1〜4モデル）"
                : "Panel models (comma separated, 1–4 models)"
            }
          >
            <Input
              required
              value={
                Array.isArray(parameters.analysis_models)
                  ? parameters.analysis_models.join(", ")
                  : ""
              }
              onChange={(event) =>
                set(
                  "analysis_models",
                  event.target.value.split(",").map((model) => model.trim()),
                )
              }
            />
          </Field>
          <Field label={ja ? "アナリストのモデル" : "Analyst model"}>
            <Input
              required
              value={String(parameters.model ?? "")}
              onChange={(event) => set("model", event.target.value)}
            />
          </Field>
          {count(
            "max_tool_calls",
            4,
            ja ? "各モデル内の最大ツール回数" : "Maximum inner tool calls",
          )}
          {count(
            "max_completion_tokens",
            8192,
            ja
              ? "各呼び出しの最大出力トークン数"
              : "Maximum completion tokens per inner call",
          )}
        </>
      )}
    </div>
  );
}

function object(value: unknown): Record<string, unknown> | undefined {
  return value && typeof value === "object" && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : undefined;
}

function argumentRecord(value: unknown): Record<string, unknown> | undefined {
  if (typeof value !== "string") return object(value);
  try {
    const parsed: unknown = JSON.parse(value);
    return object(parsed);
  } catch {
    return undefined;
  }
}

function ActivityItem({ value, ja }: { value: unknown; ja: boolean }) {
  const item = object(value);
  if (!item) return <JsonView value={value} />;
  const result = object(item.result) ?? object(item.output) ?? item;
  const citation = object(item.url_citation) ?? result;
  const action = object(item.action);
  const arguments_ = argumentRecord(item.arguments);
  const labels: Record<string, [string, string]> = {
    web_search_call: ["Web search", "Web 検索"],
    web_fetch_call: ["Page retrieval", "ページの取得"],
    datetime_call: ["Current time", "現在の時刻"],
    fusion_call: ["Fusion analysis", "Fusion の分析"],
    url_citation: ["Reported source", "報告されたソース"],
    "openrouter:web_search": ["Web search", "Web 検索"],
    "openrouter:web_fetch": ["Page retrieval", "ページの取得"],
    "openrouter:datetime": ["Current time", "現在の時刻"],
    "openrouter:fusion": ["Fusion analysis", "Fusion の分析"],
  };
  const name = typeof item.name === "string" ? item.name : "";
  const type = name in labels ? name : String(item.type ?? "");
  const label =
    labels[type]?.[ja ? 1 : 0] ??
    (type === "function_call_output"
      ? ja
        ? "Provider tool の結果"
        : "Provider tool result"
      : ja
        ? "Provider tool の活動"
        : "Provider tool activity");
  const query = action?.query ?? item.query ?? arguments_?.query;
  const url = citation.url ?? item.url ?? arguments_?.url;
  const title = citation.title ?? item.title;
  const text = result.content ?? result.datetime;
  const analysis = object(result.analysis);
  return (
    <article className="grid min-w-0 gap-2 border-l-2 border-border pl-3 text-sm">
      <strong>{label || "Provider tool"}</strong>
      {typeof query === "string" && (
        <p>
          {ja ? "検索クエリー" : "Search query"}: {query}
        </p>
      )}
      {typeof title === "string" && <p>{title}</p>}
      {typeof url === "string" && (
        <p className="break-all text-muted-foreground">{url}</p>
      )}
      {typeof text === "string" && (
        <p className="whitespace-pre-wrap">{text}</p>
      )}
      {Array.isArray(analysis?.consensus) && (
        <ul className="list-disc pl-5">
          {analysis.consensus
            .filter((point): point is string => typeof point === "string")
            .map((point, index) => (
              <li key={index}>{point}</li>
            ))}
        </ul>
      )}
      <Disclosure summary={ja ? "活動の詳細" : "Activity details"}>
        <JsonView value={value} />
      </Disclosure>
    </article>
  );
}

/** Read-only data: never convert provider URLs to Source citations. */
export function ProviderToolActivity({
  context,
  journal,
}: {
  context?: unknown;
  journal?: unknown;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const retained = Array.isArray(journal) ? journal : [];
  const history =
    context &&
    typeof context === "object" &&
    "history" in context &&
    Array.isArray(context.history)
      ? context.history
      : [];
  const activity = retained.length
    ? retained
    : history.flatMap((entry: unknown) => {
        const event = object(object(entry)?.event);
        return Array.isArray(event?.provider_activity)
          ? event.provider_activity
          : [];
      });
  if (!activity.length) return null;
  return (
    <details className="grid gap-3 rounded border border-border p-3">
      <summary>
        {ja ? "Provider tool の活動" : "Provider tool activity"}
      </summary>
      <Hint>
        {ja
          ? "OpenRouter が報告した活動です。Research の証拠や Source citation ではありません。"
          : "Activity reported by OpenRouter. This is not Research evidence or a Source citation."}
      </Hint>
      <div className="grid gap-3">
        {activity.map((entry, index) => {
          const row = object(entry);
          const items = Array.isArray(row?.items) ? row.items : [entry];
          return (
            <div
              className="grid gap-3"
              key={typeof row?.seq === "number" ? row.seq : index}
            >
              {items.map((item, itemIndex) => (
                <ActivityItem key={itemIndex} value={item} ja={ja} />
              ))}
              {typeof row?.preview === "string" && (
                <>
                  <Hint>
                    {ja
                      ? "長い結果のプレビューです。Context Journal には全文が保持されています。"
                      : "Preview of a long result. The Context Journal retains the full original."}
                  </Hint>
                  <pre className="whitespace-pre-wrap break-all text-xs">
                    {row.preview}
                  </pre>
                </>
              )}
            </div>
          );
        })}
      </div>
    </details>
  );
}
