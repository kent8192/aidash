import { useEffect, useState } from "react";
import { createClient, type GcipClientConfig } from "./gcip-sdk";
import type { Locale } from "./ui";
import { Button } from "./components/ui/button";
import { Input } from "./components/ui/input";
import { AuthCard } from "./shell/auth-card";
import { Loading, Notice } from "./components/patterns";

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
    verificationFailed:
      "Your account was created, but we couldn't send the verification email. Enter your password and resend it.",
    resend: "Resend verification email",
    resendError:
      "Unable to send the verification email. Enter your password and try again.",
    verified: "Your email is already verified. Sign in to continue.",
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
    verificationFailed:
      "アカウントは作成されましたが、確認メールを送信できませんでした。パスワードを入力して再送してください。",
    resend: "確認メールを再送",
    resendError:
      "確認メールを送信できませんでした。パスワードを入力して、もう一度お試しください。",
    verified: "メールアドレスは確認済みです。サインインしてください。",
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
  const authenticate = async (
    provider?: string,
    action: "sign-in" | "register" | "resend" = "sign-in",
  ) => {
    if (!config || !state || busy) return;
    setBusy(true);
    setMessage("");
    let client: ReturnType<typeof createClient> | undefined;
    try {
      client = createClient(config);
      if (action === "register") {
        const sent = await client.register(email, password);
        setMessage(sent ? text.verification : text.verificationFailed);
        return;
      }
      if (action === "resend") {
        const sent = await client.resend(email, password);
        setMessage(sent ? text.verification : text.verified);
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
      setMessage(action === "resend" ? text.resendError : text.error);
    } finally {
      setPassword("");
      await client?.close().catch(() => {});
      setBusy(false);
    }
  };
  const field = "grid gap-1.5 text-xs font-medium text-muted-foreground";
  return (
    <AuthCard
      title={text.title}
      titleId="sign-in-title"
      action={(["ja-JP", "en-US"] as const).map((value) => (
        <Button
          key={value}
          variant="ghost"
          size="sm"
          aria-pressed={locale === value}
          className="aria-pressed:bg-raised aria-pressed:text-foreground"
          onClick={() => setLocale(value)}
        >
          {value === "ja-JP" ? "日本語" : "English"}
        </Button>
      ))}
    >
      {!state ? (
        <form
          className="grid gap-3"
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
          <label className={field}>
            {text.organization}
            <Input
              required
              value={org}
              onChange={(event) => setOrg(event.target.value)}
              autoComplete="organization"
            />
          </label>
          <Button type="submit" size="lg">
            {text.next}
          </Button>
        </form>
      ) : config ? (
        <div className="grid gap-4">
          {config.providers.some((id) => id !== "password") && (
            <div className="grid gap-1.5">
              {config.providers
                .filter((id) => id !== "password")
                .map((id) => (
                  <Button
                    key={id}
                    variant="outline"
                    size="lg"
                    disabled={busy}
                    onClick={() => void authenticate(id)}
                  >
                    {text.provider} {id === "google.com" ? "Google" : id}
                  </Button>
                ))}
            </div>
          )}
          {config.providers.includes("password") && (
            <form
              className="grid gap-3 border-t border-border pt-4 first:border-t-0 first:pt-0"
              onSubmit={(event) => {
                event.preventDefault();
                void authenticate();
              }}
            >
              <label className={field}>
                {text.email}
                <Input
                  type="email"
                  required
                  value={email}
                  autoComplete="username"
                  onChange={(event) => setEmail(event.target.value)}
                />
              </label>
              <label className={field}>
                {text.password}
                <Input
                  type="password"
                  required
                  value={password}
                  autoComplete="current-password"
                  onChange={(event) => setPassword(event.target.value)}
                />
              </label>
              <Button type="submit" size="lg" disabled={busy}>
                {text.signIn}
              </Button>
              <div className="flex flex-wrap gap-1">
                {config.password_sign_up && (
                  <Button
                    type="button"
                    variant="ghost"
                    size="sm"
                    disabled={busy || !email || !password}
                    onClick={() => void authenticate(undefined, "register")}
                  >
                    {text.register}
                  </Button>
                )}
                <Button
                  type="button"
                  variant="ghost"
                  size="sm"
                  disabled={busy || !email || !password}
                  onClick={() => void authenticate(undefined, "resend")}
                >
                  {text.resend}
                </Button>
              </div>
            </form>
          )}
        </div>
      ) : (
        !message && <Loading>{text.loading}</Loading>
      )}
      {message && <Notice role="status">{message}</Notice>}
    </AuthCard>
  );
}
