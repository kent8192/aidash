import { useMemo, useState } from "react";
import { Validator, format } from "@cfworker/json-schema";
import type { HumanRequest } from "../types";
import { humanAnswer } from "../generated/aidash";
import { Field, useI18n } from "../ui";
import { Hint, formClass } from "../components/patterns";
import { Button } from "../components/ui/button";
import { Input } from "../components/ui/input";
import { NativeSelect } from "../components/ui/native-select";

type Primitive = string | number | boolean;
type Property = {
  type: "string" | "number" | "integer" | "boolean";
  title?: string;
  description?: string;
  enum?: Primitive[];
  const?: Primitive;
  default?: Primitive;
  minimum?: number;
  maximum?: number;
  exclusiveMinimum?: number;
  exclusiveMaximum?: number;
  multipleOf?: number;
  minLength?: number;
  maxLength?: number;
  pattern?: string;
  format?: string;
};
type Form = {
  message: string;
  schema: {
    type: "object";
    title?: string;
    description?: string;
    properties: Record<string, Property>;
    required?: string[];
  };
};

function decimalMultiple(value: number, multiple: number): boolean {
  const parts = (number: number): [bigint, number] => {
    const [coefficient, exponent = "0"] = number.toString().split("e");
    const fraction = coefficient.split(".")[1]?.length ?? 0;
    return [BigInt(coefficient.replace(".", "")), Number(exponent) - fraction];
  };
  const [valueInteger, valueExponent] = parts(value);
  const [multipleInteger, multipleExponent] = parts(multiple);
  const exponent = Math.min(valueExponent, multipleExponent);
  return (
    (valueInteger * 10n ** BigInt(valueExponent - exponent)) %
      (multipleInteger * 10n ** BigInt(multipleExponent - exponent)) ===
    0n
  );
}

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
  const [errors, setErrors] = useState<string[]>([]);
  const { form, validator } = useMemo(() => {
    const form = JSON.parse(request.prompt) as Form;
    try {
      // Native HTML constraints differ from JSON Schema (notably string
      // lengths, unanchored patterns and the origin of numeric multiples).
      // Interpret runtime schemas without eval/new Function, which desktop's
      // Content Security Policy forbids.
      for (const property of Object.values(form.schema.properties)) {
        for (const keyword of [
          "minimum",
          "maximum",
          "exclusiveMinimum",
          "exclusiveMaximum",
          "multipleOf",
        ] as const) {
          if (
            property[keyword] !== undefined &&
            (typeof property[keyword] !== "number" ||
              !Number.isFinite(property[keyword]))
          )
            throw new Error("Invalid numeric constraint");
        }
        if (property.multipleOf !== undefined && property.multipleOf <= 0)
          throw new Error("Invalid multipleOf");
        for (const keyword of ["minLength", "maxLength"] as const) {
          if (
            property[keyword] !== undefined &&
            (!Number.isInteger(property[keyword]) || property[keyword] < 0)
          )
            throw new Error("Invalid string constraint");
        }
        if (property.pattern !== undefined) new RegExp(property.pattern, "u");
        if (
          property.format !== undefined &&
          !Object.hasOwn(format, property.format)
        )
          throw new Error("Unsupported format");
      }
      const schema = {
        ...form.schema,
        properties: Object.fromEntries(
          Object.entries(form.schema.properties).map(([name, property]) => {
            const shape = { ...property };
            delete shape.multipleOf; // Checked exactly at submission below.
            return [name, shape];
          }),
        ),
      };
      return { form, validator: new Validator(schema, "2020-12", false) };
    } catch {
      // An unknown format or a pattern unavailable in this browser must not
      // leave an apparently valid form that can only fail at the server.
      return { form, validator: null };
    }
  }, [request.prompt]);
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
      noValidate
      onSubmit={(event) => {
        event.preventDefault();
        if (!validator) return;
        const data = new FormData(event.currentTarget);
        const content: Record<string, unknown> = Object.create(null);
        for (const [name, property] of Object.entries(form.schema.properties)) {
          if (property.const !== undefined) {
            content[name] = property.const;
            continue;
          }
          const raw = data.get(name);
          if (property.enum || property.type === "boolean") {
            if (typeof raw === "string" && raw !== "")
              content[name] = JSON.parse(raw);
            continue;
          }
          if (
            raw === null ||
            (raw === "" &&
              !(
                property.type === "string" &&
                form.schema.required?.includes(name)
              ))
          )
            continue;
          content[name] = ["integer", "number"].includes(property.type)
            ? Number(raw)
            : String(raw);
        }
        const result = validator.validate(content);
        // The interpreter tolerates small absolute remainders. Check decimal
        // multiples exactly so values such as 1.00000001 cannot pass a step of 1.
        const multipleErrors = Object.entries(form.schema.properties).flatMap(
          ([name, property]) => {
            const value = content[name];
            return typeof value === "number" &&
              Number.isFinite(value) &&
              property.multipleOf !== undefined &&
              !decimalMultiple(value, property.multipleOf)
              ? [
                  `${property.title ?? name}: ${ja ? "次の数の倍数を入力してください" : "Must be a multiple of"} ${property.multipleOf}`,
                ]
              : [];
          },
        );
        if (!result.valid || multipleErrors.length > 0) {
          setErrors([
            ...result.errors.map((error) => {
              const name = error.instanceLocation
                .slice(2)
                .replace(/~1/g, "/")
                .replace(/~0/g, "~");
              const label = form.schema.properties[name]?.title ?? name;
              return label ? `${label}: ${error.error}` : error.error;
            }),
            ...multipleErrors,
          ]);
          return;
        }
        setErrors([]);
        answer("accept", content);
      }}
    >
      <p className="whitespace-pre-wrap text-sm">{form.message}</p>
      {form.schema.title && (
        <p className="text-sm font-medium">{form.schema.title}</p>
      )}
      {form.schema.description && <Hint>{form.schema.description}</Hint>}
      <Hint>
        {ja
          ? "秘密情報や認証情報を入力しないでください。回答はこのMCPサーバーへ送信されます。"
          : "Do not enter secrets or credentials. Your answer is sent to this MCP server."}
      </Hint>
      {!validator && (
        <p role="alert" className="text-sm text-destructive">
          {ja
            ? "このフォームの制約はこのブラウザーでは検証できません。辞退またはキャンセルしてください。"
            : "This browser cannot validate this form's constraints. Decline or cancel the request."}
        </p>
      )}
      {errors.length > 0 && (
        <div role="alert" className="text-sm text-destructive">
          {errors.map((error, index) => (
            <p key={index}>{error}</p>
          ))}
        </div>
      )}
      {validator &&
        Object.entries(form.schema.properties).map(([name, property]) => (
          <div key={name} className="grid gap-1.5">
            <Field label={property.title ?? name}>
              {property.const !== undefined ? (
                <Input name={name} value={String(property.const)} readOnly />
              ) : property.enum || property.type === "boolean" ? (
                <NativeSelect
                  name={name}
                  required={form.schema.required?.includes(name)}
                  defaultValue={
                    property.default === undefined
                      ? ""
                      : JSON.stringify(property.default)
                  }
                >
                  <option value="">{ja ? "選択してください" : "Select"}</option>
                  {(property.enum ?? [true, false]).map((value) => (
                    <option
                      key={JSON.stringify(value)}
                      value={JSON.stringify(value)}
                    >
                      {value === ""
                        ? ja
                          ? "空の文字列"
                          : "Empty string"
                        : String(value)}
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
                  defaultValue={
                    property.default === undefined
                      ? ""
                      : String(property.default)
                  }
                />
              )}
            </Field>
            {property.description && <Hint>{property.description}</Hint>}
            {property.format && (
              <Hint>
                {ja ? "形式" : "Format"}: {property.format}
              </Hint>
            )}
          </div>
        ))}
      <div className="flex flex-wrap gap-2">
        <Button type="submit" disabled={busy || !validator}>
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
