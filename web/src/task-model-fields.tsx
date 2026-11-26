import { useEffect, useRef, useState } from "react";
import { useQuery } from "@tanstack/react-query";
import { ApprovalReviewPanel } from "./approval-review";
import { Dialog, DialogContent, DialogTitle } from "./components/ui/dialog";
import { Hint, Alert } from "./components/patterns";
import { NativeSelect } from "./components/ui/native-select";
import {
  OpenRouterModelPicker,
  ReasoningSlider,
} from "./openrouter-model-picker";
import { useNodes } from "./nodes";
import {
  openrouterModels,
  providerCredentialBindingList,
  providerCredentialGet,
} from "./generated/aidash";
import { ApiError, dashboardContext, transportGeneration } from "./transport";
import { Field, useI18n } from "./ui";
import type { State, EntityRef } from "./types";
import {
  allowedEfforts,
  defaultEffort,
  needsTaskModel,
  perMillion,
  receivingModels,
  resolveChoice,
  type ModelChoice,
  type TaskModel,
  type TaskModelFields,
  type Effort,
} from "./task-model";
import { compareModels, eligibleModel } from "./model-order";

function reasons(detail: Record<string, unknown>, ja: boolean) {
  const labels: Record<string, [string, string]> = {
    "The current authority cannot use this model": [
      "現在の権限ではこのモデルを使用できません。",
      "Your current permissions do not allow this model.",
    ],
    missing_openrouter_key: [
      "Tenant の OpenRouter キーを接続してください。",
      "Connect the Tenant’s OpenRouter key.",
    ],
    no_zdr_route: [
      "データ保持要件を満たす経路がありません。",
      "No route meets the Tenant’s retention requirements.",
    ],
    provider_credential_missing: [
      "プロバイダーの認証情報が必要です。",
      "Connect the provider credential.",
    ],
    reasoning_effort_unsupported: [
      "このモデルは選択した推論強度に対応していません。",
      "The model does not support this reasoning effort.",
    ],
  };
  return (Array.isArray(detail.reasons) ? detail.reasons : [])
    .map((reason) => {
      const code =
        typeof reason === "string"
          ? reason
          : String(reason?.message ?? reason?.code ?? "");
      return labels[code]?.[ja ? 0 : 1] ?? code.replaceAll("_", " ");
    })
    .join(" ");
}

export function taskModelError(error: unknown, ja: boolean) {
  if (error instanceof ApiError && error.detail?.code === "task_model_unusable")
    return new Error(reasons(error.detail, ja) || error.message);
  return error;
}

function OpenRouterBlocker({ tenant }: { tenant: string }) {
  const { locale } = useI18n();
  const bindings = useQuery({
    queryKey: ["provider-credential-bindings", tenant],
    queryFn: () => providerCredentialBindingList(encodeURIComponent(tenant)),
    retry: false,
  });
  const bound = bindings.data?.find(
    (b) => b.provider === "openrouter",
  )?.provider_credential_id;
  const record = useQuery({
    queryKey: ["provider-credentials", tenant, "bound", bound],
    queryFn: () => providerCredentialGet(encodeURIComponent(tenant), bound!),
    enabled: !!bound,
    retry: false,
  });
  if (
    !bindings.data ||
    (bound && (!record.data || record.data.state === "active"))
  )
    return null;
  return (
    <Hint>
      {locale === "ja-JP"
        ? "作業を始めるには、この Tenant に有効な OpenRouter キーを接続してください。"
        : "Connect an active OpenRouter key to this Tenant before starting work."}
    </Hint>
  );
}

// Keep the caller's original submission pending while the person reviews approval.
// Retry the same resolved reference and payload only after a successful approval.
export function useTaskModel(
  data: State,
  agent: State["registry"][number] | undefined,
  requestedNode?: string,
  initial?: TaskModel,
) {
  const { locale, t } = useI18n();
  const { selected } = useNodes();
  const node = requestedNode ?? selected?.name ?? data.node.id;
  const ja = locale === "ja-JP";
  const tenant =
    data.access.kind === "subject" ? data.access.tenant : undefined;
  const identity = JSON.stringify([tenant, node, agent?.id, agent?.version]);
  const [picked, setPicked] = useState<{
    identity: string;
    choice?: ModelChoice;
  }>();
  const saved: ModelChoice | undefined = initial && {
    entry: initial.model,
    effort: initial.reasoning_effort,
    registrationKey: "",
  };
  const choice = picked?.identity === identity ? picked.choice : saved;
  const [generation, resetPicker] = useState(0);
  const [pending, setPending] = useState<{
    model: EntityRef;
    identity: string;
    approve: boolean;
    message: string;
  }>();
  const decision = useRef<((approved: boolean) => void) | undefined>(undefined);
  const active = useRef<symbol | undefined>(undefined);
  useEffect(() => {
    active.current = Symbol(identity);
    return () => {
      active.current = undefined;
      decision.current?.(false);
      decision.current = undefined;
    };
  }, [identity]);
  const finish = (approved: boolean) => {
    decision.current?.(approved);
    decision.current = undefined;
    setPending(undefined);
  };
  const registration = useRef<{
    key: string;
    choice: ModelChoice;
    ref?: EntityRef;
  }>(undefined);
  const required = needsTaskModel(agent);
  const run = async <T,>(
    action: (fields: TaskModelFields) => Promise<T>,
  ): Promise<T> => {
    const context = active.current;
    if (required && !choice) throw new Error(t("modelChoose"));
    const fields: TaskModelFields = {};
    if (required && choice) {
      if (
        !registration.current ||
        registration.current.key !== choice.registrationKey
      ) {
        registration.current = { key: choice.registrationKey, choice };
      }
      const attempt = registration.current;
      const model =
        choice.entry ??
        attempt.ref ??
        (await resolveChoice(attempt.choice)).model;
      attempt.ref = model;
      fields.task_model = {
        model: { id: model.id, version: model.version },
        reasoning_effort: choice.effort,
      };
    }
    const send = async () => {
      if (!context || context !== active.current)
        throw new Error(
          ja
            ? "選択した Agent が変わりました。"
            : "The selected agent changed.",
        );
      try {
        const result = await action(fields);
        if (required) {
          setPicked({ identity });
          resetPicker((n) => n + 1);
        }
        return result;
      } catch (e) {
        throw taskModelError(e, ja);
      }
    };
    try {
      return await send();
    } catch (e) {
      if (
        !(e instanceof ApiError) ||
        e.detail?.code !== "task_model_not_approved" ||
        !fields.task_model ||
        context !== active.current ||
        !tenant
      )
        throw e;
      const detail = e.detail;
      setPending({
        identity,
        model: fields.task_model.model,
        approve: detail.can_approve === true,
        message: reasons(detail, ja),
      });
      if (
        await new Promise<boolean>((resolve) => {
          decision.current = resolve;
        })
      )
        return send();
      throw new Error(
        ja ? "モデルの承認が必要です。" : "Model approval is required.",
      );
    }
  };
  const fields = required && (
    <OpenRouterModelPicker
      key={identity + generation}
      tenant={tenant}
      entries={data.registry}
      value={choice}
      onChoice={(choice) => setPicked({ identity, choice })}
    />
  );
  const dialog = pending?.identity === identity && tenant && (
    <Dialog
      open
      onOpenChange={(open) => {
        if (!open) finish(false);
      }}
    >
      <DialogContent aria-describedby={undefined} closeLabel={t("close")}>
        <DialogTitle>
          {pending.approve
            ? ja
              ? `このモデル(${pending.model.id})はこの Tenant でまだ承認されていません。承認しますか？`
              : `Approve ${pending.model.id} for this Tenant?`
            : ja
              ? "承認依頼を送る"
              : "Request model approval"}
        </DialogTitle>
        {(choice?.catalog ||
          data.registry.some(
            (e) =>
              e.id === pending.model.id &&
              e.version === pending.model.version &&
              e.config.provider_credential === "openrouter",
          )) && <OpenRouterBlocker tenant={tenant} />}
        {pending.message && <Hint>{pending.message}</Hint>}
        <ApprovalReviewPanel
          tenant={tenant}
          node={node}
          root={pending.model}
          onApproved={pending.approve ? () => finish(true) : undefined}
        />
      </DialogContent>
    </Dialog>
  );
  return { fields, dialog, run, ready: !required || !!choice };
}

export function useRemoteTaskModel(
  task: string,
  node: string | undefined,
  agent: EntityRef | undefined,
  enabled: boolean | "inspect",
) {
  const { locale, t, local } = useI18n();
  const key = JSON.stringify([
    dashboardContext(),
    transportGeneration(),
    task,
    node,
    agent?.id,
    agent?.version,
  ]);
  const list = useQuery({
    queryKey: ["remote-task-models", key],
    enabled: !!enabled && !!node && !!agent,
    queryFn: () => receivingModels(task, node!, agent!),
    retry: false,
  });
  const [pick, setPick] = useState<{ key: string; model: TaskModel }>();
  const choice = pick?.key === key ? pick.model : undefined;
  const catalog = useQuery({
    queryKey: ["openrouter-models"],
    queryFn: () => openrouterModels(),
    staleTime: 300_000,
    enabled: !!list.data?.length,
    retry: false,
  });
  const options = (list.data ?? [])
    .map((option) => {
      const meta = catalog.data?.models?.find(
        (m) => m.id === option.openrouter_model_id,
      );
      return {
        ...meta,
        ...option,
        id: option.openrouter_model_id,
        name: local(option.name),
      };
    })
    .filter(eligibleModel)
    .sort(compareModels);
  const selected = options.find(
    (m) =>
      m.model.id === choice?.model.id &&
      m.model.version === choice.model.version,
  );
  // Older prepared intents expose only an Agent reference. A fixed-model
  // agent has no model discovery permission. Grant admission still validates it.
  const fixed =
    enabled === "inspect" &&
    list.error instanceof ApiError &&
    list.error.status === 403;
  const required = !!enabled && !fixed;
  const ready = !required || (!!selected && !list.isError);
  const levels = allowedEfforts(selected);
  const effort = defaultEffort(
    levels,
    choice?.reasoning_effort ?? selected?.reasoning_effort,
  );
  const select = (model: EntityRef, effort?: Effort) =>
    setPick({ key, model: { model, reasoning_effort: effort } });
  const fields = required && (
    <>
      {list.isError && (
        <Alert retry={() => void list.refetch()}>{t("modelLoadError")}</Alert>
      )}
      <Field label={t("model")}>
        <NativeSelect
          required
          value={selected ? JSON.stringify(selected.model) : ""}
          onChange={(event) => {
            const item = options.find(
              (m) => JSON.stringify(m.model) === event.target.value,
            );
            if (!item) {
              setPick(undefined);
              return;
            }
            select(
              item.model,
              defaultEffort(allowedEfforts(item), item.reasoning_effort),
            );
          }}
        >
          <option value="">{t("choose")}</option>
          {options.map((m) => (
            <option
              key={JSON.stringify(m.model)}
              value={JSON.stringify(m.model)}
            >
              {m.name} · {m.id} · {t("modelPricing")}{" "}
              {perMillion(m.pricing?.prompt) ?? "—"} /{" "}
              {perMillion(m.pricing?.completion) ?? "—"} USD
            </option>
          ))}
        </NativeSelect>
      </Field>
      {selected && (
        <ReasoningSlider
          levels={levels}
          value={effort}
          onChange={(e) => select(selected.model, e)}
        />
      )}
      {list.isLoading && (
        <Hint>
          {locale === "ja-JP"
            ? "実行先の承認済みモデルを取得中"
            : "Loading approved models from the receiving node"}
        </Hint>
      )}
    </>
  );
  return {
    fields,
    ready,
    payload:
      required && selected
        ? { task_model: { model: selected.model, reasoning_effort: effort } }
        : {},
  };
}
