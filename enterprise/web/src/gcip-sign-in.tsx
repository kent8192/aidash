import { useEffect, useRef, useState } from "react";
import {
  createClient,
  type GcipClient,
  type GcipClientConfig,
  type TotpEnrollment,
} from "./gcip-sdk";
import { clearEnrollIntent, hasEnrollIntent } from "./gcip-intent";
import {
  ENROLLMENT_REQUIRED,
  SECOND_FACTOR_REQUIRED,
  SECOND_FACTOR_UNSUPPORTED,
} from "./gcip-second-factor";
import { clearGcipStorage } from "./gcip-storage";
import type { Locale } from "@/ui";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { AuthCard } from "@/shell/auth-card";
import { Loading, Notice } from "@/components/patterns";
import { clearEmailHint, EMAIL_KEY, takeEmailHint } from "./gcip-email-hint";

const copy = {
  "en-US": {
    title: "Sign in to Aidash",
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
    notFound:
      "We couldn't find a sign-in for that entry. Check it, or contact your administrator.",
    challengeTitle: "Two-step verification",
    challengeHint:
      "Enter the 6-digit code from your authenticator app to finish signing in.",
    enrollTitle: "Set up two-step verification",
    enrollHint:
      "Your organization requires a second step. Add this key to an authenticator app, then enter the 6-digit code it shows.",
    enrollOptional:
      "Add this key to an authenticator app, then enter the 6-digit code it shows.",
    secret: "Setup key",
    openApp: "Open in authenticator app",
    preparing: "Preparing two-step verification…",
    code: "Verification code",
    verify: "Verify",
    cancel: "Cancel",
    startOver: "Start over",
    wrongCode: "That code is not correct. Check the app and try again.",
    expired: "This verification expired. Start over to sign in again.",
    tooManyAttempts:
      "Too many attempts. Wait a few minutes, then enter a new code.",
    unsupportedFactor:
      "Your account has a second step this page can't complete. Contact your administrator.",
    notEnabled:
      "Two-step verification isn't enabled for your organization. Contact your administrator.",
    codeFailed: "Verification failed. Please try again.",
  },
  "ja-JP": {
    title: "Aidash にサインイン",
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
    notFound:
      "サインイン先が見つかりません。入力内容を確認するか、管理者に連絡してください。",
    challengeTitle: "2 段階認証",
    challengeHint:
      "認証アプリに表示される 6 桁のコードを入力して、サインインを完了してください。",
    enrollTitle: "2 段階認証を設定",
    enrollHint:
      "組織で 2 段階認証が必須です。このキーを認証アプリに追加し、表示される 6 桁のコードを入力してください。",
    enrollOptional:
      "このキーを認証アプリに追加し、表示される 6 桁のコードを入力してください。",
    secret: "設定キー",
    openApp: "認証アプリで開く",
    preparing: "2 段階認証を準備中…",
    code: "確認コード",
    verify: "確認",
    cancel: "キャンセル",
    startOver: "最初からやり直す",
    wrongCode:
      "コードが正しくありません。アプリを確認して、もう一度お試しください。",
    expired: "確認の有効期限が切れました。最初からサインインし直してください。",
    tooManyAttempts:
      "試行回数が多すぎます。数分待ってから、新しいコードを入力してください。",
    unsupportedFactor:
      "このアカウントには、このページで完了できない認証手段があります。管理者に連絡してください。",
    notEnabled:
      "組織で 2 段階認証が有効になっていません。管理者に連絡してください。",
    codeFailed: "確認できませんでした。もう一度お試しください。",
  },
} as const;
type Message =
  | "verification"
  | "verificationFailed"
  | "resendError"
  | "verified"
  | "error";
type CodeError =
  | "wrongCode"
  | "expired"
  | "tooManyAttempts"
  | "unsupportedFactor"
  | "notEnabled"
  | "codeFailed";
// Where a sign-in stands when GCIP still wants a second factor.
type Step = "challenge" | "enroll";
function codeOf(error: unknown) {
  return typeof error === "object" && error !== null && "code" in error
    ? error.code
    : undefined;
}
function stepOf(error: unknown): Step | undefined {
  const code = codeOf(error);
  if (code === SECOND_FACTOR_REQUIRED) return "challenge";
  return code === ENROLLMENT_REQUIRED ? "enroll" : undefined;
}
// A restart is the only way forward when the pending sign-in itself is gone.
const RESTART: CodeError[] = ["expired", "unsupportedFactor", "notEnabled"];
function codeErrorOf(error: unknown): CodeError {
  switch (codeOf(error)) {
    case "auth/invalid-verification-code":
      return "wrongCode";
    case "auth/code-expired":
    case "auth/invalid-multi-factor-session":
    case "auth/missing-multi-factor-session":
    case "auth/user-token-expired":
    case "auth/requires-recent-login":
      return "expired";
    case "auth/too-many-requests":
      return "tooManyAttempts";
    case SECOND_FACTOR_UNSUPPORTED:
      return "unsupportedFactor";
    case "auth/operation-not-allowed":
    case "auth/unsupported-first-factor":
      return "notEnabled";
    default:
      return "codeFailed";
  }
}
// Exchange a fresh GCIP ID token for the opaque Aidash session.
async function exchange(state: string, token: string) {
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
  return result.return_to;
}
async function release(client: GcipClient | undefined) {
  try {
    await client?.close();
  } finally {
    await clearGcipStorage();
  }
}
async function federated(
  client: GcipClient,
  provider: string,
  loginHint?: string,
) {
  try {
    return await client.popup(provider, loginHint);
  } catch (error) {
    const blocked =
      typeof error === "object" &&
      error !== null &&
      "code" in error &&
      error.code === "auth/popup-blocked";
    if (!blocked) throw error;
    // The URL keeps the transaction state across the provider round trip.
    return client.redirect(provider, loginHint);
  }
}
export default function GcipSignIn({
  locale,
  setLocale,
}: {
  locale: Locale;
  setLocale: (locale: Locale) => void;
}) {
  const text = copy[locale];
  const params = new URLSearchParams(window.location.search);
  const state = params.get("state");
  const returnTo = params.get("return_to");
  const notFound = params.get("error") === "not_found";
  // Read during the first render so it happens before any await. Without a
  // state, discovery failed (or never ran), so any handoff left is stale.
  const [loginHint] = useState(() => {
    if (state) return takeEmailHint();
    clearEmailHint();
    return "";
  });
  // Settings asks to enroll a second factor through this same sign-in. Like the
  // email hint, a leftover intent without a transaction is stale.
  const [enroll] = useState(() => {
    if (state) return hasEnrollIntent();
    clearEnrollIntent();
    return false;
  });
  const [config, setConfig] = useState<GcipClientConfig>();
  // A client whose first factor succeeded stays open while GCIP awaits a code.
  const pending = useRef<GcipClient>(undefined);
  const [step, setStep] = useState<Step>();
  const [enrollment, setEnrollment] = useState<TotpEnrollment>();
  const [code, setCode] = useState("");
  const [codeError, setCodeError] = useState<CodeError>();
  const [email, setEmail] = useState(loginHint);
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  // Store the copy key, not localized text, so the redirect recovery effect
  // never depends on the locale and a language switch re-renders the notice.
  const [message, setMessage] = useState<Message>();
  // Keep the client for the second factor, or report that it must be closed.
  const hold = (client: GcipClient, error: unknown) => {
    const next = stepOf(error);
    if (!next) return false;
    pending.current = client;
    setStep(next);
    setEnrollment(undefined);
    setCode("");
    setCodeError(undefined);
    if (next === "enroll")
      client.startEnrollment().then(setEnrollment, (failure) => {
        setCodeError(codeErrorOf(failure));
        void leave();
      });
    return true;
  };
  const leave = async () => {
    const client = pending.current;
    pending.current = undefined;
    setStep(undefined);
    setEnrollment(undefined);
    await release(client).catch(() => {});
  };
  // Close a client left pending on unmount. Without one this must not touch
  // storage: a redirect return still has to read Firebase's pending flag.
  useEffect(
    () => () => {
      // The ref holds the client current at unmount, which is the point.
      const client = pending.current;
      if (client) void release(client).catch(() => {});
    },
    [],
  );
  useEffect(() => {
    if (!state) return;
    const controller = new AbortController();
    const load = async () => {
      const response = await fetch(
        `/auth/gcip/transaction?state=${encodeURIComponent(state)}`,
        {
          credentials: "same-origin",
          cache: "no-store",
          signal: controller.signal,
        },
      );
      if (!response.ok) throw new Error();
      const loaded = (await response.json()) as GcipClientConfig;
      if (controller.signal.aborted) return;
      // A redirect sign-in returns to this URL; finish it before showing the form.
      let destination: string | undefined;
      let failed = false;
      let held: unknown;
      const client = createClient(loaded, { enroll });
      try {
        const token = await client.redirectResult();
        if (token) destination = await exchange(state, token);
      } catch (error) {
        if (stepOf(error)) held = error;
        else failed = true;
      } finally {
        if (!held || controller.signal.aborted)
          await release(client).catch(() => {});
      }
      if (destination) {
        clearEnrollIntent();
        window.location.assign(destination);
        return;
      }
      if (controller.signal.aborted) return;
      setConfig(loaded);
      if (held) hold(client, held);
      else if (failed) setMessage("error");
    };
    load().catch(() => {
      if (controller.signal.aborted) return;
      // An unusable transaction (e.g. expired on redirect return) never reaches
      // a client; still drop any pending-redirect state Firebase left behind.
      void clearGcipStorage().catch(() => {});
      setMessage("error");
    });
    return () => controller.abort();
    // `hold` and `leave` only touch state and refs that outlive an effect run.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state, enroll]);
  const authenticate = async (
    provider?: string,
    action: "sign-in" | "register" | "resend" = "sign-in",
  ) => {
    if (!config || !state || busy) return;
    setBusy(true);
    setMessage(undefined);
    let client: GcipClient | undefined;
    let held = false;
    try {
      client = createClient(config, { enroll });
      if (action === "register") {
        const sent = await client.register(email, password);
        setMessage(sent ? "verification" : "verificationFailed");
        return;
      }
      if (action === "resend") {
        const sent = await client.resend(email, password);
        setMessage(sent ? "verification" : "verified");
        return;
      }
      const token = provider
        ? await federated(client, provider, loginHint || undefined)
        : await client.password(email, password);
      const destination = await exchange(state, token);
      // The exchange consumed the transaction; cleanup must not block navigation.
      clearEnrollIntent();
      await release(client).catch(() => {});
      window.location.assign(destination);
    } catch (error) {
      held = client !== undefined && hold(client, error);
      if (!held) setMessage(action === "resend" ? "resendError" : "error");
    } finally {
      setPassword("");
      if (!held) await release(client).catch(() => {});
      setBusy(false);
    }
  };
  const verify = async () => {
    const client = pending.current;
    if (!client || !state || !step || busy) return;
    setBusy(true);
    setCodeError(undefined);
    let token: string;
    try {
      token =
        step === "challenge"
          ? await client.resolveSecondFactor(code)
          : await client.finishEnrollment(code);
    } catch (error) {
      // A challenge answered with an enrollment intent continues to enrollment.
      if (step === "challenge" && stepOf(error) === "enroll") {
        hold(client, error);
        setBusy(false);
        return;
      }
      const failure = codeErrorOf(error);
      setCode("");
      setBusy(false);
      if (RESTART.includes(failure)) await leave();
      setCodeError(failure);
      return;
    }
    try {
      const destination = await exchange(state, token);
      clearEnrollIntent();
      pending.current = undefined;
      await release(client).catch(() => {});
      window.location.assign(destination);
    } catch {
      // A rejected exchange leaves nothing to retry with this client.
      await leave();
      setMessage("error");
      setBusy(false);
    }
  };
  const field = "grid gap-1.5 text-xs font-medium text-muted-foreground";
  return (
    <AuthCard
      title={
        step === "challenge"
          ? text.challengeTitle
          : step === "enroll"
            ? text.enrollTitle
            : text.title
      }
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
        <div className="grid gap-3">
          {notFound && <Notice role="alert">{text.notFound}</Notice>}
          <form
            className="grid gap-3"
            method="post"
            action="/auth/login"
            onSubmit={(event) => {
              event.preventDefault();
              if (busy) return;
              setBusy(true);
              setMessage(undefined);
              const address = email.trim();
              const body = new URLSearchParams({ email: address });
              if (returnTo) body.set("return_to", returnTo);
              // Follow the 303 here so the email handoff is stored only
              // when discovery succeeded (the redirect carries a state).
              // Errors answered directly (429, 400) never leave it behind.
              fetch("/auth/login", {
                method: "POST",
                body,
                credentials: "same-origin",
              })
                .then((response) => {
                  const next = new URL(response.url);
                  if (
                    !response.ok ||
                    !response.redirected ||
                    next.origin !== window.location.origin
                  )
                    throw new Error("discovery failed");
                  if (next.searchParams.has("state")) {
                    try {
                      sessionStorage.setItem(EMAIL_KEY, address);
                    } catch {
                      // Prefill is a convenience; continue without it.
                    }
                  }
                  window.location.assign(next.pathname + next.search);
                })
                .catch(() => {
                  setBusy(false);
                  setMessage("error");
                });
            }}
          >
            <label className={field}>
              {text.email}
              <Input
                type="email"
                name="email"
                required
                value={email}
                autoComplete="username"
                onChange={(event) => setEmail(event.target.value)}
              />
            </label>
            {returnTo && (
              <input type="hidden" name="return_to" value={returnTo} />
            )}
            <Button type="submit" size="lg" disabled={busy}>
              {text.next}
            </Button>
          </form>
        </div>
      ) : step ? (
        <form
          className="grid gap-3"
          onSubmit={(event) => {
            event.preventDefault();
            void verify();
          }}
        >
          <p className="text-sm text-muted-foreground">
            {step === "challenge"
              ? text.challengeHint
              : config?.mfa_mandatory
                ? text.enrollHint
                : text.enrollOptional}
          </p>
          {step === "enroll" &&
            (enrollment ? (
              <div className="grid gap-1.5 text-xs font-medium text-muted-foreground">
                {text.secret}
                <code
                  data-testid="totp-secret"
                  className="break-all rounded-md border border-border bg-raised px-2 py-1.5 font-mono text-sm text-foreground"
                >
                  {enrollment.secret}
                </code>
                <a className="text-foreground underline" href={enrollment.uri}>
                  {text.openApp}
                </a>
              </div>
            ) : (
              !codeError && <Loading>{text.preparing}</Loading>
            ))}
          <label className={field}>
            {text.code}
            <Input
              required
              value={code}
              inputMode="numeric"
              autoComplete="one-time-code"
              pattern="[0-9]{6}"
              maxLength={6}
              disabled={step === "enroll" && !enrollment}
              aria-invalid={codeError === "wrongCode"}
              aria-describedby={codeError ? "totp-error" : undefined}
              onChange={(event) => setCode(event.target.value.trim())}
            />
          </label>
          <Button
            type="submit"
            size="lg"
            disabled={
              busy || code.length !== 6 || (step === "enroll" && !enrollment)
            }
          >
            {text.verify}
          </Button>
          <Button
            type="button"
            variant="ghost"
            size="sm"
            disabled={busy}
            onClick={() => void leave()}
          >
            {text.cancel}
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
      {codeError && (
        <Notice role="alert" id="totp-error">
          {text[codeError]}
        </Notice>
      )}
      {message && <Notice role="status">{text[message]}</Notice>}
    </AuthCard>
  );
}
