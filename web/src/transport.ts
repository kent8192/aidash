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
    throw new Error(data.error ?? response.statusText);
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
