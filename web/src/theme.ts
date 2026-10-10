import { useEffect, useState, useSyncExternalStore } from "react";

export type ThemePreference = "dark" | "light" | "system";
export type Theme = "dark" | "light";

const STORAGE_KEY = "aidash-theme";
const query = "(prefers-color-scheme: dark)";

export function storedThemePreference(): ThemePreference {
  const value = localStorage.getItem(STORAGE_KEY);
  return value === "light" || value === "system" ? value : "dark";
}

function subscribeSystemTheme(onChange: () => void) {
  const media = window.matchMedia(query);
  media.addEventListener("change", onChange);
  return () => media.removeEventListener("change", onChange);
}

/** Theme preference (dark by default) resolved against the OS setting and applied to <html data-theme>. */
export function useTheme() {
  const [preference, setPreference] = useState(storedThemePreference);
  const systemDark = useSyncExternalStore(
    subscribeSystemTheme,
    () => window.matchMedia(query).matches,
  );
  const theme: Theme =
    preference === "system" ? (systemDark ? "dark" : "light") : preference;
  useEffect(() => {
    localStorage.setItem(STORAGE_KEY, preference);
  }, [preference]);
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
    document
      .querySelector('meta[name="theme-color"]')
      ?.setAttribute("content", theme === "dark" ? "#0d0f12" : "#f3f5f7");
  }, [theme]);
  return { preference, theme, setPreference };
}
