import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { NativeSelect } from "./components/ui/native-select";
import { useEffect, useState, type ReactNode } from "react";
import { useQueryClient } from "@tanstack/react-query";
import { Plug, RotateCw, Trash2 } from "lucide-react";
import { desktop, desktopAvailable, type DesktopSettings } from "./desktop";
import { selectConnection } from "./transport";
import { AuthCard } from "./shell/auth-card";
import { Alert } from "./components/patterns";

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
    <div className="flex min-h-0 flex-1 flex-col">
      <div
        className="flex h-10 shrink-0 items-center gap-2 border-b border-border bg-rail px-3"
        aria-label={english ? "Aidash connections" : "Aidashの接続先"}
      >
        <label className="flex min-w-0 items-center gap-2 text-xs text-muted-foreground">
          <Plug aria-hidden className="size-3.5 shrink-0" />
          {english ? "Connection" : "接続先"}
          <NativeSelect
            aria-label={english ? "Connection" : "接続先"}
            value={active ?? ""}
            disabled={busy}
            onChange={(event) => void change(event.target.value || null)}
            wrapperClassName="w-auto"
            className="h-7 max-w-[min(420px,60vw)] font-mono text-xs"
          >
            <option value="">
              {english ? "Manage connections" : "接続先を管理"}
            </option>
            {settings?.profiles.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name} · {p.origin}
              </option>
            ))}
          </NativeSelect>
        </label>
        {active && (
          <Button
            variant="ghost"
            size="sm"
            disabled={busy}
            onClick={() => void change(active)}
          >
            <RotateCw aria-hidden />
            {english ? "Reconnect" : "再接続"}
          </Button>
        )}
      </div>
      {error && (
        <div className="shrink-0 px-3 pt-3">
          <Alert>{error}</Alert>
        </div>
      )}
      {active ? (
        <div className="flex min-h-0 flex-1 flex-col" key={generation}>
          {children}
        </div>
      ) : (
        <AuthCard title={english ? "Connect to Aidash" : "Aidashに接続"}>
          <p className="text-muted-foreground">
            {english
              ? "Choose a running Aidash installation. Remote connections require HTTPS."
              : "起動済みのAidashを選択してください。リモート接続にはHTTPSが必要です。"}
          </p>
          <form
            className="grid gap-3"
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
            <label className="grid gap-1.5 text-xs font-medium text-muted-foreground">
              {english ? "Name" : "名前"}
              <Input
                required
                maxLength={80}
                value={name}
                onChange={(e) => setName(e.target.value)}
              />
            </label>
            <label className="grid gap-1.5 text-xs font-medium text-muted-foreground">
              URL
              <Input
                required
                type="url"
                value={origin}
                className="font-mono"
                onChange={(e) => setOrigin(e.target.value)}
              />
            </label>
            <Button disabled={busy} className="justify-self-start">
              {english ? "Save and connect" : "保存して接続"}
            </Button>
          </form>
          {settings && settings.profiles.length > 0 && (
            <ul className="divide-y divide-border border-t border-border">
              {settings.profiles.map((p) => (
                <li key={p.id} className="flex items-center gap-2 py-2.5">
                  <span className="min-w-0 flex-1">
                    <strong className="block truncate text-[13px] font-medium">
                      {p.name}
                    </strong>
                    <span className="block truncate font-mono text-[11px] text-faint">
                      {p.origin}
                    </span>
                  </span>
                  <Button
                    variant="outline"
                    size="sm"
                    disabled={busy}
                    onClick={() => void change(p.id)}
                  >
                    {english ? "Connect" : "接続"}
                  </Button>
                  <Button
                    variant="ghost"
                    size="icon"
                    className="size-7"
                    aria-label={
                      english ? "Remove saved connection" : "保存した接続先を削除"
                    }
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
                    <Trash2 aria-hidden />
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </AuthCard>
      )}
    </div>
  );
}
