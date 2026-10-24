export type Phase =
  | "sign-in"
  | "choose-account"
  | "prove-existing"
  | "prove-current"
  | "prove-target"
  | "prove-other";
export interface LoginContext {
  csrf: string;
  tenant_ids: string[];
  selected_tenant_id: string;
  providers: string[];
  password_sign_up: boolean;
  mfa_mandatory: boolean;
  client_id: string;
  phase: Phase;
  relink_only: boolean;
  firebase: { project_id: string; api_key: string; auth_domain: string };
}
export interface Account {
  account_id: string;
  source_identity_id: string;
  csrf: string;
  identities: {
    id: string;
    issuer: string;
    subject: string;
    linked: boolean;
  }[];
}
export interface Next {
  redirect_to?: string;
  phase?: Phase;
}
export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}
export async function request<T>(path: string, body?: unknown): Promise<T> {
  const response = await fetch(path, {
    credentials: "include",
    ...(body === undefined
      ? {}
      : {
          method: "POST",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify(body),
        }),
  });
  if (!response.ok) {
    let message = `This request could not be completed (${response.status}). Start sign-in again from your product.`;
    try {
      const result: unknown = await response.json();
      if (
        result &&
        typeof result === "object" &&
        "error" in result &&
        typeof result.error === "string"
      )
        message = result.error;
    } catch {
      /* A proxy can return a non-JSON error. */
    }
    throw new ApiError(response.status, message);
  }
  return response.json() as Promise<T>;
}
export function navigate(target: string) {
  const url = new URL(target, location.origin);
  const loopback = (host: string) =>
    ["localhost", "127.0.0.1", "[::1]"].includes(host);
  const localDevelopment =
    location.protocol === "http:" &&
    loopback(location.hostname) &&
    loopback(url.hostname);
  if (
    url.username ||
    url.password ||
    (url.protocol !== "https:" &&
      url.origin !== location.origin &&
      !localDevelopment)
  ) {
    throw new Error("The sign-in response contains an unsupported redirect.");
  }
  if (!["http:", "https:"].includes(url.protocol))
    throw new Error("The sign-in response contains an unsupported redirect.");
  location.assign(url.href);
}
