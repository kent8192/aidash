import en from "./locales/en-US.json";
import ja from "./locales/ja-JP.json";
export const AUTHENTICATION_EXPIRED = "aidash:unauthorized";

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
  headers.set(
    "Authorization",
    `Bearer ${sessionStorage.getItem("aidash-token") ?? ""}`,
  );
  const response = await fetch(url, { ...options, headers });
  if (!response.ok) {
    const data = await response
      .json()
      .catch(() => ({ error: response.statusText }));
    const messages =
      localStorage.getItem("aidash-locale") === "en-US" ? en : ja;
    if (response.status === 401) {
      // A response from a previous login must not expire the new session.
      if (
        headers.get("Authorization") ===
        `Bearer ${sessionStorage.getItem("aidash-token") ?? ""}`
      ) {
        window.dispatchEvent(
          new CustomEvent(AUTHENTICATION_EXPIRED, {
            detail: messages.credentialInvalid,
          }),
        );
      }
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
