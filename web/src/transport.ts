import en from "./locales/en-US.json";
import ja from "./locales/ja-JP.json";
export const AUTHENTICATION_EXPIRED = "aidash:unauthorized";
const CONTEXT_KEY = "aidash-context";
let contextGeneration = 0;

export function dashboardContext(): string | null {
  return sessionStorage.getItem(CONTEXT_KEY);
}

export function selectDashboardContext(value: string | null): void {
  contextGeneration += 1;
  if (value) sessionStorage.setItem(CONTEXT_KEY, value);
  else sessionStorage.removeItem(CONTEXT_KEY);
}

export function csrfToken(): string | null {
  return document.cookie
    .split(";")
    .map((part) => part.trim())
    .find((part) => part.startsWith("aidash-csrf="))
    ?.slice("aidash-csrf=".length) ?? null;
}

export class ApiError extends Error {
  status: number;
  constructor(message: string, status: number) {
    super(message);
    this.status = status;
  }
}

/** Authentication and error handling shared by the generated Orval clients. */
export async function authenticatedFetch(
  url: string,
  options: RequestInit = {},
): Promise<Response> {
  const headers = new Headers(options.headers);
  const context = dashboardContext();
  const generation = contextGeneration;
  if (context) headers.set("x-aidash-context", context);
  if (!["GET", "HEAD", "OPTIONS"].includes((options.method ?? "GET").toUpperCase())) {
    const csrf = csrfToken();
    if (csrf) headers.set("x-aidash-csrf", csrf);
  }
  const response = await fetch(url, { ...options, headers, credentials: "same-origin" });
  // A response from a tab's previous authority must never populate its new cache.
  if (generation !== contextGeneration || context !== dashboardContext()) {
    throw new ApiError("Authority context changed", 409);
  }
  if (!response.ok) {
    const data = await response
      .json()
      .catch(() => ({ error: response.statusText }));
    const messages =
      localStorage.getItem("aidash-locale") === "en-US" ? en : ja;
    if (response.status === 401) {
      window.dispatchEvent(
        new CustomEvent(AUTHENTICATION_EXPIRED, {
          detail: messages.credentialInvalid,
        }),
      );
      throw new ApiError(messages.credentialInvalid, response.status);
    }
    if (response.status === 403)
      throw new ApiError(messages.accessDenied, response.status);
    throw new ApiError(data.error ?? response.statusText, response.status);
  }
  return response;
}
export async function apiFetch<T>(
  url: string,
  options: RequestInit = {},
): Promise<T> {
  const response = await authenticatedFetch(url, options);
  if (response.headers.get("content-type")?.startsWith("text/event-stream")) {
    return response as T;
  }
  return response.json() as Promise<T>;
}
