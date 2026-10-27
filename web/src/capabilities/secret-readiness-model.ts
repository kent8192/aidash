/** Readiness facts for Tenant Secrets (decision 5 of #310). A fact never
 *  carries a value, a Fingerprint or the last four characters. */
export type SecretReadinessKind = "tool" | "external_service" | "slot";
export type SecretReadinessStatus =
  | "ready"
  | "secret_missing"
  | "secret_hosts_mismatch"
  | "secrets_unavailable_on_deployment"
  | "broker_not_configured";
export type SecretReadinessRepair =
  | "manage_secret"
  | "ask_administrator"
  | "deployment_administrator"
  | "none";

export type SecretReadinessFact = {
  item: string;
  kind: SecretReadinessKind;
  tenant_secret_name?: string | null;
  status: SecretReadinessStatus;
  repair: SecretReadinessRepair;
};

export type SecretReadinessLocale = "en-US" | "ja-JP";

export const secretReadinessCopy = {
  "en-US": {
    title: "Tenant Secret readiness",
    help: "Each bound tool, External service and Secret slot is checked against this Tenant's secrets. Values are never shown.",
    ready: "Ready",
    secretMissing: "Tenant Secret missing",
    hostsMismatch: "Hosts do not permit this destination",
    unavailable: "Tenant Secrets are unavailable on this Deployment",
    brokerMissing: "The Tenant Secret broker is not configured",
    manage: "Manage Tenant Secrets",
    askAdministrator:
      "Ask an administrator who can manage this Tenant's secrets to fix it.",
    deploymentAdministrator: "A Deployment Administrator must fix this.",
    secret: "Tenant Secret",
    tool: "Tool",
    externalService: "External service",
    slot: "Secret slot",
  },
  "ja-JP": {
    title: "Tenant Secret の準備状況",
    help: "バインドされたツール、External service、Secret スロットをこの Tenant の Secret と照合します。値は表示しません。",
    ready: "準備完了",
    secretMissing: "Tenant Secret がありません",
    hostsMismatch: "許可ホストが送信先と一致しません",
    unavailable: "この Deployment では Tenant Secret を利用できません",
    brokerMissing: "Tenant Secret broker が未設定です",
    manage: "Tenant Secret を管理",
    askAdministrator:
      "この Tenant の Secret を管理できる管理者に修正を依頼してください。",
    deploymentAdministrator:
      "この Deployment の Deployment Administrator が修正する必要があります。",
    secret: "Tenant Secret",
    tool: "ツール",
    externalService: "External service",
    slot: "Secret スロット",
  },
} as const;

export function readinessStatusLabel(
  status: SecretReadinessStatus,
  locale: SecretReadinessLocale,
): string {
  const copy = secretReadinessCopy[locale];
  return {
    ready: copy.ready,
    secret_missing: copy.secretMissing,
    secret_hosts_mismatch: copy.hostsMismatch,
    secrets_unavailable_on_deployment: copy.unavailable,
    broker_not_configured: copy.brokerMissing,
  }[status];
}

export function readinessKindLabel(
  kind: SecretReadinessKind,
  locale: SecretReadinessLocale,
): string {
  const copy = secretReadinessCopy[locale];
  return {
    tool: copy.tool,
    external_service: copy.externalService,
    slot: copy.slot,
  }[kind];
}

export function readinessTone(
  status: SecretReadinessStatus,
): "success" | "warning" | "danger" | "neutral" {
  return status === "ready"
    ? "success"
    : status === "secret_missing" || status === "secret_hosts_mismatch"
      ? "warning"
      : "danger";
}
