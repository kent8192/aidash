import { useState } from "react";
import { bytesFromGiB, millisFromCores } from "./scratch-settings";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { ApiError, dashboardContext, transportGeneration } from "./transport";
import {
  scratchList,
  scratchLimits,
  scratchSettings,
  scratchMemberAdd,
  scratchMemberRemove,
  scratchClear,
  scratchDelete,
} from "./generated/aidash";
import type {
  ScratchSettings as Settings,
  ScratchSpace as Space,
  ScratchLimits as Limits,
} from "./generated/models";
import { useI18n } from "./ui";
import { useNodes } from "./nodes";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { Textarea } from "./components/ui/textarea";
import { Field } from "./ui";
import { Hint, Notice } from "./components/patterns";

const gib = 1024 ** 3;
const split = (value: string) =>
  value
    .split("\n")
    .map((s) => s.trim())
    .filter(Boolean);

function Edit({
  space,
  limits,
  refresh,
}: {
  space: Space;
  limits: Limits;
  refresh: () => void;
}) {
  const { t } = useI18n();
  const [settings, setSettings] = useState(space.settings);
  const [revision, setRevision] = useState(space.revision);
  const [hosts, setHosts] = useState(
    settings.allowed_hosts.kind === "hosts"
      ? settings.allowed_hosts.hosts.join("\n")
      : "",
  );
  const [services, setServices] = useState(
    settings.external_services.join("\n"),
  );
  const [member, setMember] = useState("");
  const [error, setError] = useState<"save" | "conflict" | null>(null);
  const [saving, setSaving] = useState(false);
  const change = <K extends keyof Settings>(key: K, value: Settings[K]) =>
    setSettings((old) => ({ ...old, [key]: value }));
  const act = async (operation: () => Promise<unknown>) => {
    setSaving(true);
    setError(null);
    try {
      await operation();
      refresh();
    } catch (reason) {
      setError(
        reason instanceof ApiError && reason.status === 409
          ? "conflict"
          : "save",
      );
    } finally {
      setSaving(false);
    }
  };
  return (
    <div className="grid gap-3">
      <Field label={t("scratchCapacity")}>
        <Input
          type="number"
          min={1}
          max={limits.ceilings.capacity_bytes / gib}
          value={settings.capacity_bytes / gib}
          onChange={(e) =>
            change("capacity_bytes", bytesFromGiB(e.target.value))
          }
        />
      </Field>
      <Field label={t("scratchIdle")}>
        <Input
          type="number"
          min={1}
          max={365}
          step={1}
          value={settings.idle_expiry_days ?? ""}
          placeholder={t("scratchNever")}
          onChange={(e) =>
            change(
              "idle_expiry_days",
              e.target.value ? Math.round(Number(e.target.value)) : null,
            )
          }
        />
      </Field>
      <Field label={t("scratchHosts")}>
        <Textarea
          value={hosts}
          disabled={settings.allowed_hosts.kind === "all"}
          onChange={(e) => setHosts(e.target.value)}
        />
      </Field>
      <Hint>{t("scratchHostsWithinTenant")}</Hint>
      {(settings.allowed_hosts.kind === "all" ||
        (limits.ceilings.all_hosts_allowed &&
          limits.tenant_hosts.kind === "all")) && (
        <label>
          <input
            type="checkbox"
            checked={settings.allowed_hosts.kind === "all"}
            onChange={(e) =>
              change(
                "allowed_hosts",
                e.target.checked
                  ? { kind: "all" }
                  : { kind: "hosts", hosts: split(hosts) },
              )
            }
          />{" "}
          {t("scratchAllHosts")}
        </label>
      )}
      <Field label={t("scratchImage")}>
        <Input
          value={settings.execution_image ?? ""}
          placeholder={t("scratchImageDefault")}
          onChange={(e) => change("execution_image", e.target.value || null)}
        />
      </Field>
      <Hint>{t("scratchImageChange")}</Hint>
      <Field label={t("scratchServices")}>
        <Textarea
          value={services}
          onChange={(e) => setServices(e.target.value)}
        />
      </Field>
      <Hint>{t("scratchServicesShared")}</Hint>
      <Field label={t("scratchCpu")}>
        <Input
          type="number"
          min={0.001}
          max={limits.ceilings.cpu_millis / 1000}
          step={0.001}
          value={settings.cpu_millis / 1000}
          onChange={(e) =>
            change("cpu_millis", millisFromCores(e.target.value))
          }
        />
      </Field>
      <Field label={t("scratchMemory")}>
        <Input
          type="number"
          min={1}
          max={limits.ceilings.memory_bytes / gib}
          value={settings.memory_bytes / gib}
          onChange={(e) => change("memory_bytes", bytesFromGiB(e.target.value))}
        />
      </Field>
      <Field label={t("scratchSent")}>
        <Input
          type="number"
          min={1 / gib}
          step="any"
          max={limits.ceilings.daily_sent_bytes / gib}
          value={settings.daily_sent_bytes / gib}
          onChange={(e) =>
            change("daily_sent_bytes", bytesFromGiB(e.target.value))
          }
        />
      </Field>
      <Field label={t("scratchReceived")}>
        <Input
          type="number"
          min={1 / gib}
          step="any"
          max={limits.ceilings.daily_received_bytes / gib}
          value={settings.daily_received_bytes / gib}
          onChange={(e) =>
            change("daily_received_bytes", bytesFromGiB(e.target.value))
          }
        />
      </Field>
      <label>
        <input
          type="checkbox"
          disabled={!limits.ceilings.docker_allowed}
          checked={settings.docker_enabled && limits.ceilings.docker_allowed}
          onChange={(e) => change("docker_enabled", e.target.checked)}
        />{" "}
        {t("scratchDocker")}
      </label>
      {!limits.ceilings.docker_allowed && (
        <Hint>{t("scratchDockerUnavailable")}</Hint>
      )}
      <Button
        disabled={saving}
        onClick={() =>
          void act(() =>
            scratchSettings(space.id, {
              revision,
              settings: {
                ...settings,
                docker_enabled:
                  settings.docker_enabled && limits.ceilings.docker_allowed,
                allowed_hosts:
                  settings.allowed_hosts.kind === "all"
                    ? { kind: "all" }
                    : { kind: "hosts", hosts: split(hosts) },
                external_services: split(services),
              },
            }).then((result) => {
              setRevision((current) => current + 1);
              return result;
            }),
          )
        }
      >
        {t("scratchSave")}
      </Button>
      <div>
        {t("scratchCreator")}: {space.creator}
      </div>
      <div>
        <strong>{t("scratchMembers")}</strong>
        {space.members.map((subject) => (
          <div key={subject}>
            {subject}{" "}
            <Button
              disabled={saving}
              onClick={() =>
                void act(() => scratchMemberRemove(space.id, { subject }))
              }
            >
              {t("scratchRemove")}
            </Button>
          </div>
        ))}
      </div>
      <Field label={t("scratchMember")}>
        <Input value={member} onChange={(e) => setMember(e.target.value)} />
      </Field>
      <Button
        disabled={saving || !member.trim()}
        onClick={() =>
          void act(() => scratchMemberAdd(space.id, { subject: member.trim() }))
        }
      >
        {t("scratchAdd")}
      </Button>
      <Hint>{t("scratchClearExplanation")}</Hint>
      <Button
        disabled={saving}
        onClick={() => void act(() => scratchClear(space.id))}
      >
        {t("scratchClear")}
      </Button>
      <Button
        disabled={saving}
        onClick={() => void act(() => scratchDelete(space.id))}
      >
        {t("scratchDelete")}
      </Button>
      {error && (
        <Notice role="alert">
          {t(error === "conflict" ? "scratchConflict" : "scratchSaveError")}
          {error === "conflict" && (
            <Button onClick={() => window.location.reload()}>
              {t("scratchReload")}
            </Button>
          )}
        </Notice>
      )}
    </div>
  );
}
export function ScratchSpacesSettings() {
  const { t } = useI18n();
  const { selected } = useNodes();
  const node = selected?.name;
  const [id, setId] = useState("");
  const client = useQueryClient();
  const list = useQuery({
    queryKey: [
      "scratch-spaces",
      node,
      dashboardContext(),
      transportGeneration(),
    ],
    enabled: !!node,
    queryFn: () => scratchList({ node: node ?? "" }),
  });
  const limits = useQuery({
    queryKey: [
      "scratch-limits",
      node,
      dashboardContext(),
      transportGeneration(),
    ],
    enabled: !!node,
    queryFn: () => scratchLimits({ node: node ?? "" }),
  });
  const space = list.data?.find((value) => value.id === id);
  return (
    <section className="grid gap-3">
      <h2>{t("scratchSpace")}</h2>
      <Notice>{t("scratchOutsideGuarantees")}</Notice>
      <Field label={t("scratchChoose")}>
        <NativeSelect value={id} onChange={(e) => setId(e.target.value)}>
          <option value="">{t("scratchChoose")}</option>
          {list.data?.map((s) => (
            <option key={s.id} value={s.id}>
              {s.name} · {s.id.slice(0, 8)}
            </option>
          ))}
        </NativeSelect>
      </Field>
      {(list.isError || limits.isError) && (
        <Notice role="alert">{t("scratchLoadError")}</Notice>
      )}
      {space && limits.data && (
        <Edit
          key={`${node}:${space.id}`}
          space={space}
          limits={limits.data}
          refresh={() =>
            void client.invalidateQueries({
              queryKey: ["scratch-spaces", node],
            })
          }
        />
      )}
    </section>
  );
}
