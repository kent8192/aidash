import { useEffect, useState } from "react";
import { createClient, type GcipClientConfig } from "./gcip-sdk";
import type { Locale } from "./ui";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";

const copy = {
  "en-US": {
    title: "Sign in to Aidash",
    organization: "Organization",
    next: "Continue",
    email: "Email",
    password: "Password",
    signIn: "Sign in",
    register: "Create account",
    verification: "Check your email to verify your account, then sign in.",
    provider: "Continue with",
    error: "Sign-in failed. Please try again.",
    loading: "Loading sign-in…",
  },
  "ja-JP": {
    title: "Aidash にサインイン",
    organization: "組織名",
    next: "続ける",
    email: "メールアドレス",
    password: "パスワード",
    signIn: "サインイン",
    register: "アカウントを作成",
    verification:
      "確認メールからアカウントを確認してから、サインインしてください。",
    provider: "続ける",
    error: "サインインできませんでした。もう一度お試しください。",
    loading: "サインインを準備中…",
  },
} as const;
export default function GcipSignIn({
  locale,
  setLocale,
}: {
  locale: Locale;
  setLocale: (locale: Locale) => void;
}) {
  const text = copy[locale];
  const state = new URLSearchParams(window.location.search).get("state");
  const [config, setConfig] = useState<GcipClientConfig>();
  const [org, setOrg] = useState("");
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState("");
  useEffect(() => {
    if (!state) return;
    const controller = new AbortController();
    fetch(`/auth/gcip/transaction?state=${encodeURIComponent(state)}`, {
      credentials: "same-origin",
      cache: "no-store",
      signal: controller.signal,
    })
      .then(async (response) => {
        if (!response.ok) throw new Error();
        return response.json() as Promise<GcipClientConfig>;
      })
      .then(setConfig)
      .catch(() => {
        if (!controller.signal.aborted) setMessage(text.error);
      });
    return () => controller.abort();
  }, [state, text.error]);
  const authenticate = async (provider?: string, register = false) => {
    if (!config || !state || busy) return;
    setBusy(true);
    setMessage("");
    let client: ReturnType<typeof createClient> | undefined;
    try {
      client = createClient(config);
      if (register) {
        await client.register(email, password);
        setMessage(text.verification);
        return;
      }
      const token = provider
        ? await client.popup(provider)
        : await client.password(email, password);
      const response = await fetch("/auth/gcip/exchange", {
        method: "POST",
        credentials: "same-origin",
        cache: "no-store",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({ state, id_token: token }),
      });
      if (!response.ok) throw new Error();
      const result: { return_to: string } = await response.json();
      // The server owns the destination; keep a final same-origin path guard.
      if (
        !result.return_to.startsWith("/") ||
        result.return_to.startsWith("//") ||
        result.return_to.includes("\\")
      )
        throw new Error();
      await client.close();
      window.location.assign(result.return_to);
    } catch {
      setMessage(text.error);
    } finally {
      setPassword("");
      await client?.close().catch(() => {});
      setBusy(false);
    }
  };
  return (
    <main className="gcip-sign-in" aria-labelledby="sign-in-title">
      <div>
        <Button variant="ghost" onClick={() => setLocale("ja-JP")}>
          日本語
        </Button>
        <Button variant="ghost" onClick={() => setLocale("en-US")}>
          English
        </Button>
      </div>
      <h1 id="sign-in-title">{text.title}</h1>
      {!state ? (
        <form
          onSubmit={(event) => {
            event.preventDefault();
            const query = new URLSearchParams({ org });
            const destination = new URLSearchParams(window.location.search).get(
              "return_to",
            );
            if (destination) query.set("return_to", destination);
            window.location.assign(`/auth/login?${query}`);
          }}
        >
          <label>
            {text.organization}
            <Input
              required
              value={org}
              onChange={(event) => setOrg(event.target.value)}
              autoComplete="organization"
            />
          </label>
          <Button type="submit">{text.next}</Button>
        </form>
      ) : config ? (
        <>
          {config.providers
            .filter((id) => id !== "password")
            .map((id) => (
              <Button
                key={id}
                disabled={busy}
                onClick={() => void authenticate(id)}
              >
                {text.provider} {id === "google.com" ? "Google" : id}
              </Button>
            ))}
          {config.providers.includes("password") && (
            <form
              onSubmit={(event) => {
                event.preventDefault();
                void authenticate();
              }}
            >
              <label>
                {text.email}
                <Input
                  type="email"
                  required
                  value={email}
                  autoComplete="username"
                  onChange={(event) => setEmail(event.target.value)}
                />
              </label>
              <label>
                {text.password}
                <Input
                  type="password"
                  required
                  value={password}
                  autoComplete="current-password"
                  onChange={(event) => setPassword(event.target.value)}
                />
              </label>
              <Button type="submit" disabled={busy}>
                {text.signIn}
              </Button>
              {config.password_sign_up && (
                <Button
                  type="button"
                  disabled={busy || !email || !password}
                  onClick={() => void authenticate(undefined, true)}
                >
                  {text.register}
                </Button>
              )}
            </form>
          )}
        </>
      ) : (
        !message && <p role="status">{text.loading}</p>
      )}
      {message && <p role="status">{message}</p>}
    </main>
  );
}
