import { useEffect, useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { desktop, desktopAvailable, type DesktopSettings } from "./desktop";
import { selectConnection } from "./transport";

/** Desktop integration lives at the application boundary, outside shared screens. */
export function ConnectionGate({
  children,
  english,
}: {
  children: ReactNode;
  english: boolean;
}) {
  const client = useQueryClient();
  const [settings, setSettings] = useState<DesktopSettings | null>(null);
  const [active, setActive] = useState<string | null>(null);
  const [generation, setGeneration] = useState(0);
  const [name, setName] = useState("");
  const [origin, setOrigin] = useState("http://127.0.0.1:8080");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState("");
  const native = desktopAvailable();
  useEffect(() => {
    if (!native) return;
    let cancelled = false;
    void desktop
      .settings()
      .then((value) => {
        if (cancelled) return;
        setSettings(value);
        const profile = value.profiles.find((p) => p.id === value.selected);
        selectConnection(profile ?? null);
        setActive(profile?.id ?? null);
      })
      .catch((reason) => {
        if (!cancelled) setError(String(reason));
      });
    return () => {
      cancelled = true;
    };
  }, [native]);
  if (!native) return <>{children}</>;
  const change = async (id: string | null) => {
    setBusy(true);
    setError("");
    // Immediately fence the old renderer and streams before awaiting native work.
    setActive(null);
    selectConnection(null);
    await client.cancelQueries();
    client.clear();
    try {
      await desktop.select(id);
      const value = await desktop.settings();
      setSettings(value);
      const profile = value.profiles.find((p) => p.id === id);
      selectConnection(profile ?? null);
      setActive(profile?.id ?? null);
      setGeneration((value) => value + 1);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="desktop-shell">
      <div
        className="desktop-connections"
        aria-label={english ? "Aidash connections" : "Aidashの接続先"}
      >
        <label>
          {english ? "Connection" : "接続先"}{" "}
          <select
            aria-label={english ? "Connection" : "接続先"}
            value={active ?? ""}
            disabled={busy}
            onChange={(event) => void change(event.target.value || null)}
          >
            <option value="">
              {english ? "Manage connections" : "接続先を管理"}
            </option>
            {settings?.profiles.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name} — {p.origin}
              </option>
            ))}
          </select>
        </label>
        {active && (
          <button disabled={busy} onClick={() => void change(active)}>
            {english ? "Reconnect" : "再接続"}
          </button>
        )}
      </div>
      {error && (
        <p className="error" role="alert">
          {error}
        </p>
      )}
      {active ? (
        <div className="desktop-content" key={generation}>
          {children}
        </div>
      ) : (
        <main className="login">
          <div className="login-card">
            <h1>{english ? "Connect to Aidash" : "Aidashに接続"}</h1>
            <p>
              {english
                ? "Choose a running Aidash installation. Remote connections require HTTPS."
                : "起動済みのAidashを選択してください。リモート接続にはHTTPSが必要です。"}
            </p>
            <form
              onSubmit={(event) => {
                event.preventDefault();
                setBusy(true);
                setError("");
                void desktop
                  .save(name, origin)
                  .then(async (profile) => {
                    setName("");
                    await change(profile.id);
                  })
                  .catch((reason) => setError(String(reason)))
                  .finally(() => setBusy(false));
              }}
            >
              <label>
                {english ? "Name" : "名前"}
                <input
                  required
                  maxLength={80}
                  value={name}
                  onChange={(e) => setName(e.target.value)}
                />
              </label>
              <label>
                URL
                <input
                  required
                  type="url"
                  value={origin}
                  onChange={(e) => setOrigin(e.target.value)}
                />
              </label>
              <button disabled={busy}>
                {english ? "Save and connect" : "保存して接続"}
              </button>
            </form>
            {settings?.profiles.map((p) => (
              <div className="card" key={p.id}>
                <strong>{p.name}</strong>
                <p>{p.origin}</p>
                <button disabled={busy} onClick={() => void change(p.id)}>
                  {english ? "Connect" : "接続"}
                </button>
                <button
                  disabled={busy}
                  onClick={() => {
                    setBusy(true);
                    setError("");
                    void desktop
                      .remove(p.id)
                      .then(() => desktop.settings())
                      .then(setSettings)
                      .catch((reason) => setError(String(reason)))
                      .finally(() => setBusy(false));
                  }}
                >
                  {english ? "Remove saved connection" : "保存した接続先を削除"}
                </button>
              </div>
            ))}
          </div>
        </main>
      )}
    </div>
  );
}
