import { useRef, useState } from "react";
import { parseDocument } from "yaml";
import { apiFetch } from "./transport";
import { Field, useI18n } from "./ui";

export type SkillPayload = {
  instructions: string;
  files: { path: string; content: string; encoding?: string }[];
  source?: string;
};
type ImportResult = {
  skills: string[];
  selected: (SkillPayload & { path: string; source: string }) | null;
};

function validateSkill(text: string): string {
  if (new TextEncoder().encode(text).length > 65536) throw new Error();
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
  return `${meta.name}: ${meta.description}`;
}

export function SkillImport({
  change,
}: {
  change: (payload: SkillPayload) => void;
}) {
  const { t } = useI18n();
  const [error, setError] = useState("");
  const [name, setName] = useState("");
  const [url, setUrl] = useState("");
  const [choices, setChoices] = useState<string[]>([]);
  const [choice, setChoice] = useState("");
  const [busy, setBusy] = useState(false);
  const [files, setFiles] = useState<SkillPayload["files"]>([]);
  const [source, setSource] = useState("");
  const request = useRef(0);

  const clear = () => {
    setName("");
    setFiles([]);
    setSource("");
    change({ instructions: "", files: [] });
  };

  const load = async (skillPath?: string) => {
    const current = ++request.current;
    setBusy(true);
    setError("");
    clear();
    try {
      const result = await apiFetch<ImportResult>("/api/skills/import", {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ url: url.trim(), skill_path: skillPath }),
      });
      if (current !== request.current) return;
      setChoices(result.skills);
      if (result.selected) {
        const imported = result.selected;
        setName(validateSkill(imported.instructions));
        setChoice(imported.path);
        setFiles(imported.files);
        setSource(imported.source);
        change({
          instructions: imported.instructions,
          files: imported.files,
          source: imported.source,
        });
      } else {
        setChoice(result.skills[0] ?? "");
      }
    } catch (cause) {
      if (current !== request.current) return;
      setError(
        cause instanceof Error && cause.message
          ? cause.message
          : t("skillImportError"),
      );
    } finally {
      if (current === request.current) setBusy(false);
    }
  };

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
      <Field label={t("skillImportUrl")}>
        <input
          type="url"
          value={url}
          placeholder="https://skills.sh/owner/repo/skill"
          onChange={(event) => {
            request.current += 1;
            setBusy(false);
            setUrl(event.target.value);
            setChoices([]);
            setChoice("");
            clear();
          }}
        />
      </Field>
      <button
        type="button"
        disabled={busy || !url.trim()}
        onClick={() => void load()}
      >
        {busy ? t("loading") : t("skillImportFromUrl")}
      </button>
      {choices.length > 1 && (
        <>
          <Field label={t("skillImportChoose")}>
            <select
              value={choice}
              onChange={(event) => setChoice(event.target.value)}
            >
              {choices.map((path) => (
                <option key={path} value={path}>
                  {path}
                </option>
              ))}
            </select>
          </Field>
          <button
            type="button"
            disabled={busy || !choice}
            onClick={() => void load(choice)}
          >
            {t("skillImportSelected")}
          </button>
        </>
      )}
      <Field label="SKILL.md">
        <input
          type="file"
          accept=".md"
          onChange={async (event) => {
            const file = event.target.files?.[0];
            event.target.value = "";
            setError("");
            if (!file) return;
            const current = ++request.current;
            setBusy(false);
            clear();
            try {
              if (file.name !== "SKILL.md" || file.size > 65536)
                throw new Error();
              const text = await file.text();
              if (current !== request.current) return;
              setName(validateSkill(text));
              setFiles([]);
              setSource("");
              setChoices([]);
              change({ instructions: text, files: [] });
            } catch {
              if (current === request.current) setError(t("skillImportError"));
            }
          }}
        />
      </Field>
      {name && <p role="status">{name}</p>}
      {source && (
        <p>
          <a href={source} target="_blank" rel="noreferrer">
            {source}
          </a>
        </p>
      )}
      {files.length > 0 && (
        <details>
          <summary>
            {t("skillImportFiles")} ({files.length})
          </summary>
          {files.map((file) => (
            <details key={file.path}>
              <summary>{file.path}</summary>
              {file.encoding === "base64" ? (
                <p>{t("skillImportBinary")}</p>
              ) : (
                <pre>{file.content}</pre>
              )}
            </details>
          ))}
        </details>
      )}
      {error && <p role="alert">{error}</p>}
    </fieldset>
  );
}
