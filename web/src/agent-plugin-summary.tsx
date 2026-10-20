import {
  describeComponent,
  describeServer,
  httpsLink,
  type AgentPluginRef,
  type Finding,
  type FindingOutcome,
  type McpServer,
} from "./agent-plugins";
import { Facts, Hint, type Fact } from "./components/patterns";
import { Badge, useI18n } from "./ui";

const subheading = "text-xs font-semibold text-foreground";
const componentLabels = {
  manifest: "agentPluginManifest",
  mcp: "agentPluginMcp",
  skill: "agentPluginSkill",
  server: "agentPluginServer",
} as const;

function Server({ name, server }: { name: string; server: McpServer }) {
  const { t } = useI18n();
  const description = describeServer(server);
  const facts: Fact[] =
    description.transport === "stdio"
      ? [
          [
            t("agentPluginCommand"),
            <>
              {description.command}{" "}
              <span className="font-sans text-muted-foreground">
                (
                {t(
                  description.fromPlugin
                    ? "agentPluginFromPlugin"
                    : "agentPluginFromRuntime",
                )}
                )
              </span>
            </>,
            true,
          ],
          [t("agentPluginArgs"), description.args.join(" ") || "-", true],
          [
            t("agentPluginEnv"),
            description.env.length > 0
              ? description.env.map(({ name, value }) => (
                  <div key={name}>
                    {name}={value}
                  </div>
                ))
              : "-",
            true,
          ],
          [t("agentPluginCwd"), description.cwd, true],
        ]
      : [
          [t("agentPluginServerUrl"), description.url, true],
          [
            t("agentPluginHeaders"),
            description.headers.join(", ") || "-",
            true,
          ],
        ];
  return (
    <li className="agent-plugin-server grid min-w-0 gap-1.5 border-l-2 border-border pl-3">
      <div className="flex min-w-0 flex-wrap items-center gap-2">
        <span className="font-mono text-xs font-medium text-foreground [overflow-wrap:anywhere]">
          {name}
        </span>
        <Badge
          value={
            description.transport === "stdio"
              ? "agentPluginStdio"
              : "agentPluginRemote"
          }
          tone={description.transport === "stdio" ? "warning" : "neutral"}
        />
      </div>
      <Facts items={facts} />
    </li>
  );
}

function Findings({
  outcome,
  findings,
}: {
  outcome: FindingOutcome;
  findings: Finding[];
}) {
  const { t } = useI18n();
  if (findings.length === 0) return null;
  return (
    <div className="grid gap-1">
      <h5 className={subheading}>
        {t(outcome === "ignored" ? "agentPluginIgnored" : "agentPluginSkipped")}{" "}
        ({findings.length})
      </h5>
      <Hint>
        {t(
          outcome === "ignored"
            ? "agentPluginIgnoredHelp"
            : "agentPluginSkippedHelp",
        )}
      </Hint>
      <ul className="grid gap-1.5 text-xs">
        {findings.map((finding, index) => {
          const { kind, name } = describeComponent(finding.component);
          return (
            <li
              key={index}
              className="min-w-0 text-muted-foreground [overflow-wrap:anywhere]"
            >
              <span className="font-medium text-foreground">
                {t(componentLabels[kind])}
                {name && <span className="font-mono"> {name}</span>}
              </span>{" "}
              · <span className="font-mono">{finding.reason}</span>
              {finding.detail && <> · {finding.detail}</>}
            </li>
          );
        })}
      </ul>
    </div>
  );
}

/**
 * What an Agent Plugin brings into an Agent: its Skills, the servers it may
 * start or call, and everything left out. The importer shows it before the
 * plugin is added, and the Catalog Approval screen shows it before the
 * version is approved. `denied` is known only at import.
 */
export function AgentPluginSummary({
  plugin,
  denied = [],
}: {
  plugin: AgentPluginRef;
  denied?: string[];
}) {
  const { t } = useI18n();
  const link = httpsLink(plugin.source);
  const servers = Object.entries(plugin.servers);
  const facts: Fact[] = [[t("name"), plugin.name, true]];
  if (plugin.version) facts.push([t("version"), plugin.version, true]);
  if (plugin.license) facts.push([t("agentPluginLicense"), plugin.license]);
  facts.push(
    [
      t("agentPluginSource"),
      link ? (
        <a
          href={link}
          target="_blank"
          rel="noreferrer"
          className="text-brand underline-offset-4 hover:underline"
        >
          {plugin.source}
        </a>
      ) : (
        plugin.source
      ),
      true,
    ],
    [t("agentPluginSkills"), plugin.skills.join(", ") || t("agentPluginNone")],
  );
  return (
    <div className="agent-plugin grid min-w-0 gap-3">
      <Facts items={facts} />
      {servers.length > 0 && (
        <div className="grid gap-1.5">
          <h5 className={subheading}>
            {t("agentPluginServers")} ({servers.length})
          </h5>
          <ul className="grid gap-2.5">
            {servers.map(([name, server]) => (
              <Server key={name} name={name} server={server} />
            ))}
          </ul>
        </div>
      )}
      {(["ignored", "skipped"] as const).map((outcome) => (
        <Findings
          key={outcome}
          outcome={outcome}
          findings={plugin.findings.filter((item) => item.outcome === outcome)}
        />
      ))}
      {denied.length > 0 && (
        <div className="grid gap-1">
          <h5 className={subheading}>
            {t("agentPluginDenied")} ({denied.length})
          </h5>
          <ul className="grid gap-0.5 font-mono text-xs text-muted-foreground">
            {denied.map((path) => (
              <li key={path} className="[overflow-wrap:anywhere]">
                {path}
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  );
}
