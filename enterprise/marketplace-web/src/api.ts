export interface Plugin {
  id: string;
  publisher_id: string;
  publisher_name: string;
  slug: string;
  name: string;
  description: string;
  license: string;
  origin: string;
  price_cents: number;
  currency: string;
  declared_permissions: string[];
  latest_version: string | null;
}

export interface PluginVersion {
  version: string;
  digest: string;
  size_bytes: number;
  recalled: boolean;
  recall_reason: string | null;
  origin?: string;
}

export interface Session {
  authenticated: boolean;
  account_id?: string;
  csrf_token?: string;
}

export interface Publisher {
  id: string;
  name: string;
}
export interface LibraryEntry {
  plugin_id: string;
  slug: string;
  name: string;
  versions: PluginVersion[];
}

export class ApiError extends Error {
  constructor(
    public status: number,
    message: string,
  ) {
    super(message);
  }
}

export async function request<T>(
  path: string,
  options: RequestInit = {},
): Promise<T> {
  const response = await fetch(path, { ...options, credentials: "include" });
  if (!response.ok) {
    let message = `Request failed (${response.status}). Please try again.`;
    try {
      const body: unknown = await response.json();
      if (
        body &&
        typeof body === "object" &&
        "error" in body &&
        typeof body.error === "string"
      )
        message = body.error;
    } catch {
      /* Keep the HTTP error when a proxy returns a non-JSON response. */
    }
    throw new ApiError(response.status, message);
  }
  if (response.status === 204) return undefined as T;
  return response.json() as Promise<T>;
}

export function write<T>(
  path: string,
  body: unknown,
  session: Session,
): Promise<T> {
  if (!session.authenticated || !session.csrf_token) {
    return Promise.reject(
      new ApiError(401, "Sign in again before making this change."),
    );
  }
  return request<T>(path, {
    method: "POST",
    headers: {
      "Content-Type": "application/json",
      "X-Marketplace-CSRF": session.csrf_token,
    },
    body: JSON.stringify(body),
  });
}

export function price(
  plugin: Pick<Plugin, "price_cents" | "currency">,
): string {
  if (plugin.price_cents === 0) return "Free";
  try {
    return new Intl.NumberFormat("en", {
      style: "currency",
      currency: plugin.currency,
    }).format(plugin.price_cents / 100);
  } catch {
    return `${plugin.price_cents / 100} ${plugin.currency}`;
  }
}

export function downloadPath(id: string, version: string): string {
  return `/api/marketplace/plugins/${encodeURIComponent(id)}/versions/${encodeURIComponent(version)}/download`;
}
