import { useState } from "react";
import type { HumanRequest } from "../types";
import { humanAnswer } from "../generated/aidash";
import { Field, useI18n } from "../ui";
import { Hint, formClass } from "../components/patterns";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { NativeSelect } from "../components/ui/native-select";

type Property = {
  type: string;
  title?: string;
  description?: string;
  enum?: (string | number)[];
  minimum?: number;
  maximum?: number;
  minLength?: number;
  maxLength?: number;
};
type Form = {
  message: string;
  schema: { properties: Record<string, Property>; required?: string[] };
};

export function elicitationMessage(request: HumanRequest): string {
  if (request.kind !== "MCP_ELICITATION") return request.prompt;
  try {
    return (JSON.parse(request.prompt) as Form).message;
  } catch {
    return request.prompt;
  }
}

/** The server validates the same primitive schema and rejects stale answers. */
export function ElicitationForm({
  request,
  submit,
}: {
  request: HumanRequest;
  submit: (action: () => Promise<unknown>) => unknown;
}) {
  const { locale } = useI18n();
  const ja = locale === "ja-JP";
  const [busy, setBusy] = useState(false);
  const form = JSON.parse(request.prompt) as Form;
  const answer = (action: string, content?: Record<string, unknown>) => {
    if (busy) return;
    setBusy(true);
    void submit(async () => {
      try {
        return await humanAnswer(request.id, {
          action,
          ...(content ? { content } : {}),
        });
      } finally {
        setBusy(false);
      }
    });
  };
  return (
    <form
      className={formClass}
      onSubmit={(event) => {
        event.preventDefault();
        const data = new FormData(event.currentTarget);
        const content: Record<string, unknown> = Object.create(null);
        for (const [name, property] of Object.entries(form.schema.properties)) {
          const raw = data.get(name);
          if (raw === null || raw === "") continue;
          content[name] =
            property.type === "boolean"
              ? raw === "true"
              : ["integer", "number"].includes(property.type)
                ? Number(raw)
                : String(raw);
        }
        answer("accept", content);
      }}
    >
      <p className="whitespace-pre-wrap text-sm">{form.message}</p>
      <Hint>
        {ja
          ? "秘密情報や認証情報を入力しないでください。回答はこのMCPサーバーへ送信されます。"
          : "Do not enter secrets or credentials. Your answer is sent to this MCP server."}
      </Hint>
      {Object.entries(form.schema.properties).map(([name, property]) => (
        <div key={name} className="grid gap-1.5">
          <Field label={property.title ?? name}>
            {property.enum || property.type === "boolean" ? (
              <NativeSelect
                name={name}
                required={form.schema.required?.includes(name)}
                defaultValue=""
              >
                <option value="">{ja ? "選択してください" : "Select"}</option>
                {(property.enum ?? ["true", "false"]).map((value) => (
                  <option key={String(value)} value={String(value)}>
                    {String(value)}
                  </option>
                ))}
              </NativeSelect>
            ) : (
              <Input
                name={name}
                required={form.schema.required?.includes(name)}
                type={
                  ["integer", "number"].includes(property.type)
                    ? "number"
                    : "text"
                }
                step={property.type === "integer" ? 1 : "any"}
                min={property.minimum}
                max={property.maximum}
                minLength={property.minLength}
                maxLength={property.maxLength}
              />
            )}
          </Field>
          {property.description && <Hint>{property.description}</Hint>}
        </div>
      ))}
      <div className="flex flex-wrap gap-2">
        <Button type="submit" disabled={busy}>
          {ja ? "送信" : "Accept"}
        </Button>
        <Button
          type="button"
          variant="outline"
          disabled={busy}
          onClick={() => answer("decline")}
        >
          {ja ? "辞退" : "Decline"}
        </Button>
        <Button
          type="button"
          variant="ghost"
          disabled={busy}
          onClick={() => answer("cancel")}
        >
          {ja ? "キャンセル" : "Cancel"}
        </Button>
      </div>
    </form>
  );
}
