import { useState, type ReactNode } from "react";
import { Input } from "./components/ui/input";
import { Hint, pairClass } from "./components/patterns";
import { Field, useI18n } from "./ui";
import {
  defaultEmbedding,
  embeddingRefusal,
  useTenantLevel,
  useZdrRoutes,
} from "./embedding-defaults";

/**
 * The model and dimensions of an OpenRouter embedding definition, prefilled
 * for the Tenant's Data Protection Level, with a warning when the model cannot
 * meet it (#963). Remounts once the level is known so the prefill follows it.
 */
export function EmbeddingModelFields({
  tenant,
  modelLabel,
  modelName,
  dimensionsName,
  dimensionsLabel,
  versionField,
  previous,
}: {
  tenant: string | undefined;
  modelLabel: string;
  modelName: string;
  dimensionsName: string;
  dimensionsLabel: string;
  versionField: ReactNode;
  previous?: { model: string; dimensions: number };
}) {
  const level = useTenantLevel(tenant);
  return (
    <Fields
      key={level ?? "unknown"}
      level={level}
      modelLabel={modelLabel}
      modelName={modelName}
      dimensionsName={dimensionsName}
      dimensionsLabel={dimensionsLabel}
      versionField={versionField}
      initial={previous ?? defaultEmbedding(level)}
    />
  );
}

function Fields({
  level,
  modelLabel,
  modelName,
  dimensionsName,
  dimensionsLabel,
  versionField,
  initial,
}: {
  level: ReturnType<typeof useTenantLevel>;
  modelLabel: string;
  modelName: string;
  dimensionsName: string;
  dimensionsLabel: string;
  versionField: ReactNode;
  initial: { model: string; dimensions: number };
}) {
  const { locale } = useI18n();
  const routes = useZdrRoutes();
  const [model, setModel] = useState(initial.model);
  const refusal = embeddingRefusal(model, level, routes, locale === "ja-JP");
  return (
    <>
      <Field label={modelLabel}>
        <Input
          name={modelName}
          required
          value={model}
          onChange={(event) => setModel(event.target.value)}
        />
      </Field>
      {refusal && <Hint role="alert">{refusal}</Hint>}
      <div className={pairClass}>
        {versionField}
        <Field label={dimensionsLabel}>
          <Input
            name={dimensionsName}
            type="number"
            required
            min={1}
            max={8192}
            defaultValue={initial.dimensions}
          />
        </Field>
      </div>
    </>
  );
}
