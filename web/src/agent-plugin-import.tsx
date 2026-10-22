import { useRef, useState } from "react";
import { AgentPluginSummary } from "./agent-plugin-summary";
import {
  MAX_AGENT_PLUGINS,
  agentPlugins,
  canAttach,
  readImport,
  withPlugin,
  withoutPlugin,
  type ImportedAgentPlugin,
} from "./agent-plugins";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Alert, Disclosure, Group, Hint, Notice } from "./components/patterns";
import { apiFetch } from "./transport";
import { Field, useI18n } from "./ui";

type Config = Record<string, unknown>;

/**
 * Imports an Agent Plugin from GitHub and attaches its pinned reference to the
 * Agent configuration in `config.agent_plugins` (ADR 0072). Nothing runs
 * until the Tenant approves the Agent version.
 */
export function AgentPluginImport({
  config,
  change,
}: {
  config: Config;
  change: (config: Config) => void;
}) {
  const { t } = useI18n();
  const [url, setUrl] = useState("");
  const [choices, setChoices] = useState<string[]>([]);
  const [choice, setChoice] = useState("");
  const [imported, setImported] = useState<ImportedAgentPlugin | null>(null);
  const [error, setError] = useState("");
  const [busy, setBusy] = useState(false);
  const request = useRef(0);
  const attached = agentPlugins({ kind: "agent", config }) ?? [];
  const name = imported?.reference.name;
  const replaces = attached.some((plugin) => plugin.name === name);
  const room = name !== undefined && canAttach(config, name);

  const load = async (pluginPath?: string) => {
    const current = ++request.current;
    setBusy(true);
    setError("");
    setImported(null);
    try {
      const result = readImport(
        await apiFetch<unknown>("/api/agent-plugins/import", {
          method: "POST",
          headers: { "content-type": "application/json" },
          body: JSON.stringify({ url: url.trim(), plugin_path: pluginPath }),
        }),
      );
      if (current !== request.current) return;
      if (!result) throw new Error(t("agentPluginUnreadable"));
      if (result.plugins.length > 1 || !pluginPath) setChoices(result.plugins);
      if (result.selected) {
        setChoice(pluginPath ?? result.plugins[0] ?? "");
        setImported(result.selected);
      } else if (result.plugins.length > 1) {
        setChoice(result.plugins[0]);
      } else {
        throw new Error(t("agentPluginNotFound"));
      }
    } catch (cause) {
      if (current !== request.current) return;
      setError(
        cause instanceof Error && cause.message
          ? cause.message
          : t("agentPluginError"),
      );
    } finally {
      if (current === request.current) setBusy(false);
    }
  };

  return (
    <Group legend={t("agentPluginImport")}>
      <Hint>{t("agentPluginImportHelp")}</Hint>
      <Notice>{t("agentPluginRuntime")}</Notice>
      <div className="flex min-w-0 items-end gap-2">
        <div className="min-w-0 flex-1">
          <Field label={t("agentPluginUrl")}>
            <Input
              type="url"
              value={url}
              placeholder="https://github.com/owner/repo"
              onChange={(event) => {
                request.current += 1;
                setBusy(false);
                setUrl(event.target.value);
                setChoices([]);
                setChoice("");
                setImported(null);
                setError("");
              }}
              onKeyDown={(event) => {
                if (event.key !== "Enter") return;
                event.preventDefault();
                if (!busy && url.trim()) void load();
              }}
            />
          </Field>
        </div>
        <Button
          variant="outline"
          type="button"
          disabled={busy || !url.trim()}
          onClick={() => void load()}
        >
          {busy ? t("loading") : t("agentPluginFromUrl")}
        </Button>
      </div>
      {choices.length > 1 && (
        <>
          <Field label={t("agentPluginChoose")}>
            <NativeSelect
              value={choice}
              onChange={(event) => setChoice(event.target.value)}
            >
              {choices.map((path) => (
                <option key={path} value={path}>
                  {path}
                </option>
              ))}
            </NativeSelect>
          </Field>
          <Button
            variant="outline"
            type="button"
            className="justify-self-start"
            disabled={busy || !choice}
            onClick={() => void load(choice)}
          >
            {t("agentPluginSelected")}
          </Button>
        </>
      )}
      {error && <Alert>{error}</Alert>}
      {imported && (
        <div className="grid min-w-0 gap-3 rounded-md border border-border bg-surface px-3 py-2.5">
          <AgentPluginSummary
            plugin={imported.reference}
            denied={imported.denied}
          />
          <Button
            variant="outline"
            type="button"
            className="justify-self-start"
            disabled={!room}
            onClick={() => {
              change(withPlugin(config, imported.reference));
              setImported(null);
            }}
          >
            {t(replaces ? "agentPluginReplace" : "agentPluginAdd")}
          </Button>
          {!room && <Hint>{t("agentPluginLimit")}</Hint>}
        </div>
      )}
      {attached.length > 0 && (
        <div className="grid min-w-0 gap-1.5">
          <span className="text-xs font-semibold text-foreground">
            {t("agentPluginAttached")} ({attached.length}/{MAX_AGENT_PLUGINS})
          </span>
          <ul className="grid min-w-0 gap-2">
            {attached.map((plugin) => (
              <li
                key={plugin.name}
                className="flex min-w-0 items-start gap-2 border-l-2 border-brand-line pl-3"
              >
                <Disclosure
                  className="min-w-0 flex-1"
                  summary={
                    <span className="font-mono">
                      {plugin.name}
                      {plugin.version ? ` · ${plugin.version}` : ""}
                    </span>
                  }
                >
                  <AgentPluginSummary plugin={plugin} />
                </Disclosure>
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  onClick={() => change(withoutPlugin(config, plugin.name))}
                >
                  {t("agentPluginRemove")}
                </Button>
              </li>
            ))}
          </ul>
        </div>
      )}
    </Group>
  );
}
