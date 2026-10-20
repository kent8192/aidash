import en from "./locales/en-US.json";
import ja from "./locales/ja-JP.json";
import { desktop, desktopAvailable, type ConnectionProfile } from "./desktop";
export const AUTHENTICATION_EXPIRED = "aidash:unauthorized";
export const CONNECTION_CHANGED = "aidash:connection-changed";
/** Dispatched after a change the browser session's Mappings show, such as a
 *  Tenant label or a deleted Tenant, so every context selector reads the
 *  session again. */
export const SESSION_CHANGED = "aidash:session-changed";
const CONTEXT_KEY = "aidash-context";
let contextGeneration = 0;
let activeConnection: ConnectionProfile | null = null;
let lifetime = new AbortController();

/** The context selector for a Mapping names its Tenant, so the server looks
 * the Mapping up within that Tenant only (ADR 0047). A header carries only
 * visible ASCII, so the Tenant name is percent-encoded as the server encodes
 * it: every UTF-8 byte outside visible ASCII, and `%`, becomes `%XX`. */
export function mappingContext(mapping: {
  tenant: string;
  id: string;
}): string {
  let tenant = "";
  for (const byte of new TextEncoder().encode(mapping.tenant))
    tenant +=
      byte > 0x20 && byte < 0x7f && byte !== 0x25
        ? String.fromCharCode(byte)
        : `%${byte.toString(16).toUpperCase().padStart(2, "0")}`;
  return `mapping:${tenant}:${mapping.id}`;
}
export function dashboardContext(): string | null {
  return sessionStorage.getItem(CONTEXT_KEY);
}
function invalidate(): void {
  contextGeneration += 1;
  lifetime.abort();
  lifetime = new AbortController();
}
export function selectDashboardContext(value: string | null): void {
  invalidate();
  if (value) sessionStorage.setItem(CONTEXT_KEY, value);
  else sessionStorage.removeItem(CONTEXT_KEY);
}
export function selectConnection(profile: ConnectionProfile | null): void {
  activeConnection = profile;
  selectDashboardContext(null);
  sessionStorage.removeItem("aidash-session-id");
  window.dispatchEvent(new Event(CONNECTION_CHANGED));
}
export function transportGeneration(): number {
  return contextGeneration;
}
export function csrfToken(): string | null {
  return (
    document.cookie
      .split(";")
      .map((part) => part.trim())
      .find((part) => part.startsWith("aidash-csrf="))
      ?.slice("aidash-csrf=".length) ?? null
  );
}
export class ApiError extends Error {
  status: number;
  readonly responseReceived: boolean;
  constructor(message: string, status: number, responseReceived = false) {
    super(message);
    this.status = status;
    this.responseReceived = responseReceived;
  }
}
function current(generation: number): void {
  if (generation !== contextGeneration)
    throw new ApiError("Connection or authority changed", 409);
}
/** Raw HTTP for session lifecycle calls, whose callers handle 401 themselves. */
export async function sessionFetch(
  path: string,
  options: RequestInit = {},
): Promise<Response> {
  const generation = contextGeneration;
  const connection = activeConnection;
  if (!path.startsWith("/") || path.startsWith("//") || /[\\\r\n]/.test(path))
    throw new ApiError("Invalid API path", 400);
  const base = desktopAvailable() ? connection?.origin : window.location.origin;
  if (!base) throw new ApiError("Choose an Aidash connection", 400);
  const url = new URL(path, base);
  if (
    url.origin !== new URL(base).origin ||
    !/^\/(api|auth)(\/|$)/.test(url.pathname)
  )
    throw new ApiError("Invalid API destination", 400);
  const controller = new AbortController();
  const parent = lifetime.signal;
  const abort = () => controller.abort();
  parent.addEventListener("abort", abort, { once: true });
  options.signal?.addEventListener("abort", abort, { once: true });
  if (parent.aborted || options.signal?.aborted) abort();
  const headers = new Headers(options.headers);
  // A caller naming a context, such as a deleted Tenant's Mapping to
  // restore it, keeps it; every other request carries the chosen one.
  const context = dashboardContext();
  if (context && !headers.has("x-aidash-context"))
    headers.set("x-aidash-context", context);
  const native = desktopAvailable();
  if (native) {
    headers.delete("x-aidash-csrf");
    headers.delete("authorization");
  } else if (
    !["GET", "HEAD", "OPTIONS"].includes(
      (options.method ?? "GET").toUpperCase(),
    )
  ) {
    const csrf = csrfToken();
    if (csrf) headers.set("x-aidash-csrf", csrf);
  }
  const cleanup = () => {
    parent.removeEventListener("abort", abort);
    options.signal?.removeEventListener("abort", abort);
  };
  const send = async (force: boolean) => {
    if (native) {
      const access = await desktop.access(
        force ? headers.get("authorization")?.slice(7) : undefined,
      );
      current(generation);
      if (access) headers.set("authorization", `Bearer ${access.access_token}`);
      else headers.delete("authorization");
    }
    current(generation);
    return fetch(native ? url.toString() : path, {
      ...options,
      headers,
      signal: controller.signal,
      credentials: native ? "omit" : "same-origin",
      redirect: "error",
    });
  };
  try {
    let response = await send(false);
    current(generation);
    // Only retry an explicit authentication rejection; never replay a mutation
    // after a transport failure with an unknown server outcome.
    if (native && response.status === 401 && headers.has("authorization")) {
      await response.body?.cancel();
      response = await send(true);
      current(generation);
    }
    // Keep the generation fence through body consumption, including streamed SSE.
    const bodyless =
      [204, 205, 304].includes(response.status) ||
      options.method?.toUpperCase() === "HEAD";
    if (bodyless) await response.body?.cancel();
    const reader = bodyless ? undefined : response.body?.getReader();
    const body = reader
      ? new ReadableStream<Uint8Array>({
          async pull(stream) {
            try {
              current(generation);
              const chunk = await reader.read();
              current(generation);
              if (controller.signal.aborted)
                throw new DOMException("Aborted", "AbortError");
              if (chunk.done) {
                cleanup();
                reader.releaseLock();
                stream.close();
              } else stream.enqueue(chunk.value);
            } catch (error) {
              cleanup();
              await reader.cancel().catch(() => {});
              stream.error(error);
            }
          },
          async cancel() {
            cleanup();
            await reader.cancel().catch(() => {});
          },
        })
      : null;
    if (!reader) cleanup();
    return new Response(body, {
      status: response.status,
      statusText: response.statusText,
      headers: response.headers,
    });
  } catch (error) {
    cleanup();
    throw error;
  }
}
/** Whether the session itself is still valid; it ignores the context. */
async function sessionHolds(): Promise<boolean> {
  const response = await sessionFetch("/auth/session", {
    cache: "no-store",
  }).catch(() => null);
  await response?.body?.cancel();
  return response?.ok === true;
}
export async function authenticatedFetch(
  url: string,
  options: RequestInit = {},
): Promise<Response> {
  const response = await sessionFetch(url, options);
  if (!response.ok) {
    const generation = contextGeneration;
    const data = await response
      .json()
      .catch(() => ({ error: response.statusText }));
    current(generation);
    const messages =
      localStorage.getItem("aidash-locale") === "en-US" ? en : ja;
    if (response.status === 401) {
      // A desktop token sees a deleted Tenant as unknown (ADR 0060). While
      // its session still holds, refresh the contexts instead of signing out.
      if (desktopAvailable() && dashboardContext() && (await sessionHolds())) {
        current(generation);
        window.dispatchEvent(new Event(SESSION_CHANGED));
        throw new ApiError(
          typeof data.error === "string" ? data.error : response.statusText,
          response.status,
          true,
        );
      }
      window.dispatchEvent(
        new CustomEvent(AUTHENTICATION_EXPIRED, {
          detail: messages.credentialInvalid,
        }),
      );
      throw new ApiError(messages.credentialInvalid, response.status, true);
    }
    if (response.status === 403)
      throw new ApiError(messages.accessDenied, response.status, true);
    // The chosen Tenant was deleted; the session no longer offers it.
    if (response.status === 410)
      window.dispatchEvent(new Event(SESSION_CHANGED));
    throw new ApiError(
      typeof data.error === "string"
        ? data.error
        : (data.error?.message ?? response.statusText),
      response.status,
      true,
    );
  }
  return response;
}
export async function apiFetch<T>(
  url: string,
  options: RequestInit = {},
): Promise<T> {
  const generation = contextGeneration;
  const response = await authenticatedFetch(url, options);
  if (response.headers.get("content-type")?.startsWith("text/event-stream"))
    return response as T;
  // A bodyless success, such as the 204 of a delete, has nothing to parse.
  if ([204, 205].includes(response.status)) return undefined as T;
  const result = (await response.json()) as T;
  current(generation);
  return result;
}
export async function signIn(returnTo: string): Promise<void> {
  if (desktopAvailable()) await desktop.login();
  else
    window.location.assign(
      `/auth/login?return_to=${encodeURIComponent(returnTo)}`,
    );
}
export async function signOut(allDevices: boolean): Promise<void> {
  if (desktopAvailable() && !allDevices) {
    await desktop.logout();
    return;
  }
  const response = await sessionFetch(
    allDevices ? "/auth/logout-all" : "/auth/logout",
    { method: "POST" },
  );
  if (!response.ok) throw new Error("Logout failed");
  if (desktopAvailable()) await desktop.logout();
}
