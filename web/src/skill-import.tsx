import { useState } from "react";
import { parseDocument } from "yaml";
import { Field, useI18n } from "./ui";

export function SkillImport({ change }: { change: (text: string) => void }) {
  const { t } = useI18n();
  const [error, setError] = useState("");
  const [name, setName] = useState("");
  return (
    <fieldset>
      <legend>{t("skillImport")}</legend>
      <p>{t("skillImportHelp")}</p>
      <p>
        <a
          href="https://github.com/anthropics/skills"
          target="_blank"
          rel="noreferrer"
        >
          Anthropic Skills
        </a>{" "}
        ·{" "}
        <a
          href="https://github.com/openai/skills"
          target="_blank"
          rel="noreferrer"
        >
          OpenAI Skills
        </a>
      </p>
      <Field label="SKILL.md">
        <input
          type="file"
          accept=".md"
          onChange={async (event) => {
            const file = event.target.files?.[0];
            event.target.value = "";
            setError("");
            if (!file) return;
            try {
              if (file.name !== "SKILL.md" || file.size > 65536)
                throw new Error();
              const text = await file.text();
              const match = text
                .replace(/\r\n/g, "\n")
                .match(/^---\n([\s\S]*?)\n---\n([\s\S]+)$/);
              if (!match) throw new Error();
              const parsed = parseDocument(match[1]);
              if (parsed.errors.length) throw new Error();
              const meta = parsed.toJS({ maxAliasCount: 0 });
              if (
                typeof meta?.name !== "string" ||
                !/^[a-z0-9]+(?:-[a-z0-9]+)*$/.test(meta.name) ||
                meta.name.length > 64 ||
                typeof meta.description !== "string" ||
                !meta.description.trim() ||
                meta.description.length > 1024 ||
                !match[2].trim()
              )
                throw new Error();
              change(text);
              setName(`${meta.name}: ${meta.description}`);
            } catch {
              setError(t("skillImportError"));
            }
          }}
        />
      </Field>
      {name && <p role="status">{name}</p>}
      {error && <p role="alert">{error}</p>}
    </fieldset>
  );
}
