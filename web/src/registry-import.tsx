import { useRef, useState } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { parseDocument } from "yaml";
import { registryImport } from "./generated/aidash";
import type { Entry } from "./types";
import { Field, JsonView, Modal, useI18n } from "./ui";

const MAX_BYTES = 512_000;
const bytes = (text: string) => new TextEncoder().encode(text).length;
const object = (value: unknown): value is Record<string, unknown> =>
  value !== null && typeof value === "object" && !Array.isArray(value);
const localized = (value: unknown): value is Record<string, string> =>
  object(value) &&
  Object.keys(value).length > 0 &&
  Object.values(value).every((item) => typeof item === "string");
const strings = (value: unknown): string[] => {
  if (value === undefined) return [];
  if (!Array.isArray(value) || !value.every((item) => typeof item === "string"))
    throw new Error("importJsonFormat");
  return value;
};

function parseSource(source: string, skillVersion: string): Entry[] {
  const text = source.replace(/^\uFEFF/, "").trim();
  if (text.startsWith("---")) {
    const match = /^---\r?\n([\s\S]*?)\r?\n---(?:\r?\n|$)([\s\S]*)$/.exec(text);
    if (!match) throw new Error("importSkillFormat");
    const document = parseDocument(match[1]);
    if (document.errors.length) throw new Error("importSkillFormat");
    const metadata: unknown = document.toJS({ maxAliasCount: 0 });
    if (
      !object(metadata) ||
      typeof metadata.name !== "string" ||
      typeof metadata.description !== "string" ||
      !metadata.name.trim() ||
      !metadata.description.trim() ||
      !match[2].trim()
    ) {
      throw new Error("importSkillFormat");
    }
    return [
      {
        id: metadata.name.trim(),
        version: skillVersion.trim(),
        kind: "skill",
        name: { en: metadata.name.trim() },
        description: { en: metadata.description.trim() },
        capabilities: [],
        tags: [],
        languages: [],
        skills: [],
        schema: {},
        config: { instructions: match[2].trim() },
      },
    ];
  }
  const value: unknown = JSON.parse(text);
  let entries: unknown[];
  if (Array.isArray(value)) entries = value;
  else if (
    object(value) &&
    Object.keys(value).length === 1 &&
    Array.isArray(value.entries)
  )
    entries = value.entries;
  else entries = [value];
  return entries.map((entry) => {
    if (
      !object(entry) ||
      typeof entry.id !== "string" ||
      !entry.id ||
      typeof entry.version !== "string" ||
      typeof entry.kind !== "string" ||
      !localized(entry.name) ||
      !localized(entry.description) ||
      (entry.config !== undefined && !object(entry.config)) ||
      (entry.schema !== undefined && !object(entry.schema))
    )
      throw new Error("importJsonFormat");
    // Preserve unknown properties so the server can reject typos instead of silently dropping them.
    return {
      ...entry,
      id: entry.id,
      version: entry.version,
      kind: entry.kind,
      name: entry.name,
      description: entry.description,
      capabilities: strings(entry.capabilities),
      tags: strings(entry.tags),
      languages: strings(entry.languages),
      skills: strings(entry.skills),
      schema: entry.schema ?? {},
      config: entry.config ?? {},
    };
  });
}

export function RegistryImportButton() {
  const { t } = useI18n();
  const [open, setOpen] = useState(false);
  return (
    <>
      <button onClick={() => setOpen(true)}>{t("registryImport")}</button>
      {open && <ImportDialog close={() => setOpen(false)} />}
    </>
  );
}

function ImportDialog({ close }: { close: () => void }) {
  const { t, local } = useI18n();
  const client = useQueryClient();
  const [text, setText] = useState("");
  const [files, setFiles] = useState<{ name: string; text: string }[]>([]);
  const [version, setVersion] = useState("1.0.0");
  const [entries, setEntries] = useState<Entry[]>([]);
  const [error, setError] = useState("");
  const [reading, setReading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{
    imported: number;
    unchanged: number;
  }>();
  const generation = useRef(0);
  const pending = useRef(false);
  const fileInput = useRef<HTMLInputElement>(null);
  const reset = () => {
    setEntries([]);
    setError("");
    setResult(undefined);
  };
  const preview = () => {
    reset();
    try {
      const sources = files.length ? files : [{ name: "", text }];
      if (
        sources.reduce((sum, source) => sum + bytes(source.text), 0) > MAX_BYTES
      )
        throw new Error("importTooLarge");
      const parsed = sources.flatMap((source) => {
        try {
          return parseSource(source.text, version);
        } catch (error) {
          throw new Error(
            `${source.name ? `${source.name}: ` : ""}${t(error instanceof Error ? error.message : "importJsonFormat")}`,
          );
        }
      });
      if (!parsed.length || parsed.length > 100)
        throw new Error("importCountLimit");
      const identities = parsed.map((entry) => `${entry.id}@${entry.version}`);
      if (new Set(identities).size !== identities.length)
        throw new Error("importDuplicate");
      if (bytes(JSON.stringify({ entries: parsed })) > MAX_BYTES)
        throw new Error("importTooLarge");
      setEntries(parsed);
    } catch (error) {
      setError(t(error instanceof Error ? error.message : "importJsonFormat"));
    }
  };
  const save = async () => {
    if (pending.current) return;
    pending.current = true;
    setBusy(true);
    setError("");
    try {
      const response = await registryImport({ entries });
      setResult(response);
      await client.invalidateQueries();
    } catch (error) {
      setError(error instanceof Error ? error.message : String(error));
    } finally {
      pending.current = false;
      setBusy(false);
    }
  };
  return (
    <Modal title={t("registryImport")} close={close}>
      <p>{t("importHelp")}</p>
      <p className="muted">{t("importSkillHelp")}</p>
      <fieldset disabled={busy || Boolean(result)}>
        <Field label={t("importFiles")}>
          <input
            ref={fileInput}
            type="file"
            accept=".json,.md,application/json,text/markdown"
            multiple
            onChange={async (event) => {
              const current = ++generation.current;
              reset();
              setFiles([]);
              setText("");
              const selected = Array.from(event.target.files ?? []);
              if (
                selected.length > 100 ||
                selected.reduce((sum, file) => sum + file.size, 0) > MAX_BYTES
              ) {
                setReading(false);
                setError(t("importTooLarge"));
                return;
              }
              setReading(true);
              try {
                const sources = await Promise.all(
                  selected.map(async (file) => ({
                    name: file.name,
                    text: await file.text(),
                  })),
                );
                if (current === generation.current) setFiles(sources);
              } catch {
                if (current === generation.current)
                  setError(t("importReadError"));
              } finally {
                if (current === generation.current) setReading(false);
              }
            }}
          />
        </Field>
        {files.length === 0 && (
          <Field label={t("importText")}>
            <textarea
              rows={8}
              value={text}
              disabled={reading}
              onChange={(event) => {
                setText(event.target.value);
                reset();
              }}
            />
          </Field>
        )}
        {files.length > 0 && (
          <>
            <p>{files.map((file) => file.name).join(", ")}</p>
            <button
              onClick={() => {
                ++generation.current;
                setFiles([]);
                setReading(false);
                if (fileInput.current) fileInput.current.value = "";
                reset();
              }}
            >
              {t("importClearFiles")}
            </button>
          </>
        )}
        <Field label={t("importSkillVersion")}>
          <input
            value={version}
            onChange={(event) => {
              setVersion(event.target.value);
              reset();
            }}
          />
        </Field>
        <button
          disabled={reading || (!text.trim() && files.length === 0)}
          onClick={preview}
        >
          {t("importPreview")}
        </button>
      </fieldset>
      {error && (
        <div className="error" role="alert">
          {error}
        </div>
      )}
      {entries.length > 0 && (
        <>
          <h3>
            {t("importPreview")} ({entries.length})
          </h3>
          <ul>
            {entries.map((entry) => (
              <li key={`${entry.id}@${entry.version}`}>
                {local(entry.name)} — {entry.id}@{entry.version} (
                {t(entry.kind)})
              </li>
            ))}
          </ul>
          <details>
            <summary>{t("details")}</summary>
            <JsonView value={entries} />
          </details>
          <p>{t("importAtomicHelp")}</p>
          <button
            className="primary"
            disabled={busy || Boolean(result)}
            onClick={() => void save()}
          >
            {t(busy ? "loading" : "importConfirm")}
          </button>
        </>
      )}
      {result && (
        <p role="status">
          {t("importSuccess")}: {result.imported} / {t("importUnchanged")}:{" "}
          {result.unchanged}
        </p>
      )}
    </Modal>
  );
}
