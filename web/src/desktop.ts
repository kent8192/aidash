/** The only module that knows about Tauri. Shared screens use this adapter. */
export type ConnectionProfile = { id: string; name: string; origin: string };
export type DesktopSettings = {
  profiles: ConnectionProfile[];
  selected: string | null;
};
export type DesktopAccess = { access_token: string; expires_in: number };
export const desktopAvailable = () => "__TAURI_INTERNALS__" in window;
async function command<T>(
  name: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<T>(name, args);
}
export const desktop = {
  settings: () => command<DesktopSettings>("connection_settings"),
  save: (name: string, origin: string) =>
    command<ConnectionProfile>("save_connection", { name, origin }),
  select: (id: string | null) => command<void>("select_connection", { id }),
  remove: (id: string) => command<void>("remove_connection", { id }),
  login: () => command<void>("desktop_login"),
  access: (rejectedToken?: string) =>
    command<DesktopAccess | null>("desktop_access", {
      rejectedToken: rejectedToken ?? null,
    }),
  logout: () => command<void>("desktop_logout"),
};
