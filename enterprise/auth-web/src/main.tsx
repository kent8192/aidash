import {
  useEffect,
  useRef,
  useState,
  type FormEvent,
  type ReactNode,
} from "react";
import { createRoot } from "react-dom/client";
import {
  ApiError,
  navigate,
  request,
  type Account,
  type LoginContext,
  type Next,
  type Phase,
} from "./api";
import type { GcipClient, TotpEnrollment } from "./gcip-sdk";
import {
  ENROLLMENT_REQUIRED,
  SECOND_FACTOR_REQUIRED,
  SECOND_FACTOR_UNSUPPORTED,
} from "./gcip-second-factor";
import "./style.css";

function useLoad<T>(path: string) {
  const [attempt, setAttempt] = useState(0);
  const [state, setState] = useState<{
    path: string;
    attempt: number;
    data?: T;
    error?: Error;
  }>({ path, attempt: -1 });
  useEffect(() => {
    let current = true;
    request<T>(path)
      .then((data) => {
        if (current) setState({ path, attempt, data });
      })
      .catch((error: Error) => {
        if (current) setState({ path, attempt, error });
      });
    return () => {
      current = false;
    };
  }, [path, attempt]);
  const loading = state.path !== path || state.attempt !== attempt;
  return {
    data: loading ? undefined : state.data,
    error: loading ? undefined : state.error,
    loading,
    reload: () => setAttempt((value) => value + 1),
  };
}

function Frame({ children }: { children: ReactNode }) {
  return (
    <div className="layout">
      <header className="brand">
        <span className="brand-mark" aria-hidden="true">
          a
        </span>
        <strong>aidash</strong>
        <span className="brand-divider" />
        <span>account</span>
      </header>
      <main>{children}</main>
      <footer>
        One Aidash Account. Your product permissions stay with each product.
      </footer>
    </div>
  );
}
function Loading() {
  return (
    <p className="loading" role="status">
      Preparing a secure sign-in…
    </p>
  );
}
function Notice({ error, children }: { error?: boolean; children: ReactNode }) {
  return (
    <div
      className={`notice ${error ? "error" : ""}`}
      role={error ? "alert" : "status"}
    >
      {children}
    </div>
  );
}
function Failure({ error, retry }: { error: Error; retry?: () => void }) {
  return (
    <>
      <Notice error>{error.message}</Notice>
      {retry && (
        <button className="secondary" onClick={retry}>
          Try again
        </button>
      )}
    </>
  );
}

const titles: Record<Phase, string> = {
  "sign-in": "Sign in to Aidash",
  "choose-account": "Choose your Aidash Account",
  "prove-existing": "Verify your existing Account",
  "prove-current": "Verify your current Account",
  "prove-target": "Verify the new sign-in method",
  "prove-other": "Verify another sign-in method",
};
const descriptions: Record<Phase, string> = {
  "sign-in": "Use a configured sign-in method to continue to your product.",
  "choose-account":
    "Your sign-in method is verified. Choose whether to create a common Account or attach it to an Account you already use.",
  "prove-existing":
    "Sign in afresh with a method already linked to your existing Aidash Account. Matching email addresses do not link Accounts.",
  "prove-current":
    "Sign in afresh to prove the Aidash Account you are managing. Your existing SSO session is not sufficient for this change.",
  "prove-target":
    "Now sign in afresh with the method you want to add. A method previously associated with another Account cannot be transferred.",
  "prove-other":
    "Sign in afresh with a different enabled method on this Account. The method being removed cannot prove this change.",
};
function errorCode(error: unknown) {
  return error && typeof error === "object" && "code" in error
    ? error.code
    : undefined;
}
function errorMessage(error: unknown): string {
  if (error instanceof ApiError) return error.message;
  switch (errorCode(error)) {
    case "auth/invalid-verification-code":
      return "That verification code is not correct. Check your authenticator and try again.";
    case "auth/code-expired":
    case "auth/invalid-multi-factor-session":
    case "auth/missing-multi-factor-session":
      return "This verification has expired. Start this sign-in step again.";
    case "auth/too-many-requests":
      return "Too many attempts. Wait a few minutes before trying again.";
    case "auth/popup-blocked":
      return "Your browser blocked the sign-in window. Allow popups or continue in this tab.";
    case "auth/popup-closed-by-user":
      return "The sign-in window was closed. Try again when you are ready.";
    case "auth/weak-password":
      return "Choose a stronger password and try again.";
    case SECOND_FACTOR_UNSUPPORTED:
      return "This sign-in method requires a second factor this page cannot complete. Use your identity provider's recovery or contact your administrator.";
    default:
      return "Sign-in could not be completed. Check your details and try again, or use your identity provider's recovery.";
  }
}

function SignInForm({
  context,
  onProof,
  onTenant,
  restart,
}: {
  context: LoginContext;
  onProof: (token: string) => Promise<void>;
  onTenant: (tenant: string) => Promise<void>;
  restart: () => void;
}) {
  const client = useRef<GcipClient | undefined>(undefined);
  const [ready, setReady] = useState(false);
  const [busy, setBusy] = useState(false);
  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string>();
  const [notice, setNotice] = useState<string>();
  const [secondFactor, setSecondFactor] = useState<"challenge" | "enroll">();
  const [enrollment, setEnrollment] = useState<TotpEnrollment>();
  const [redirectProvider, setRedirectProvider] = useState<string>();

  async function handleFailure(caught: unknown) {
    if (errorCode(caught) === SECOND_FACTOR_REQUIRED) {
      setSecondFactor("challenge");
      setError(undefined);
      return;
    }
    if (errorCode(caught) === ENROLLMENT_REQUIRED) {
      setSecondFactor("enroll");
      setError(undefined);
      try {
        setEnrollment(await client.current!.startEnrollment());
      } catch (failure) {
        setError(errorMessage(failure));
      }
      return;
    }
    setError(errorMessage(caught));
  }
  async function complete(token: string) {
    // Upstream proof exists only transiently in this authentication origin.
    // Close GCIP before the next proof step; never persist a token in UI state.
    await client.current?.close();
    client.current = undefined;
    setReady(false);
    await onProof(token);
  }
  useEffect(() => {
    let active = true;
    let current: GcipClient | undefined;
    import("./gcip-sdk")
      .then(async ({ createClient }) => {
        if (!active) return;
        current = createClient({
          ...context.firebase,
          tenant_id: context.selected_tenant_id,
          providers: context.providers,
          password_sign_up: context.password_sign_up,
          mfa_mandatory: context.mfa_mandatory,
        });
        client.current = current;
        const token = await current.redirectResult();
        if (!active) return;
        setReady(true);
        if (token) {
          setBusy(true);
          try {
            await complete(token);
          } finally {
            if (active) setBusy(false);
          }
        }
      })
      .catch((caught) => {
        if (active) void handleFailure(caught);
      });
    return () => {
      active = false;
      void current?.close();
    };
    // A new server context remounts this form; each proof has its own client.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [context]);

  async function attempt(operation: () => Promise<void>) {
    setBusy(true);
    setError(undefined);
    setNotice(undefined);
    try {
      await operation();
    } catch (caught) {
      await handleFailure(caught);
    } finally {
      setBusy(false);
    }
  }
  async function emailAction(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    await attempt(async () => {
      await complete(await client.current!.password(email, password));
    });
  }
  async function provider(id: string) {
    setRedirectProvider(undefined);
    await attempt(async () => {
      try {
        await complete(await client.current!.popup(id));
      } catch (caught) {
        if (errorCode(caught) === "auth/popup-blocked") setRedirectProvider(id);
        throw caught;
      }
    });
  }
  async function register() {
    await attempt(async () => {
      const sent = await client.current!.register(email, password);
      setPassword("");
      setNotice(
        sent
          ? "Check your email and verify your address, then sign in to continue. This has not created an Aidash Account yet."
          : "Your login was created, but the verification email could not be sent. Enter your password and resend it.",
      );
    });
  }
  async function resend() {
    await attempt(async () => {
      const sent = await client.current!.resend(email, password);
      setPassword("");
      setNotice(
        sent
          ? "Verification email sent. Verify your address, then sign in."
          : "This email is already verified. Sign in to continue.",
      );
    });
  }
  async function verifyCode(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    const code = String(new FormData(event.currentTarget).get("code"));
    await attempt(async () => {
      await complete(
        await (secondFactor === "enroll"
          ? client.current!.finishEnrollment(code)
          : client.current!.resolveSecondFactor(code)),
      );
    });
  }
  async function changeTenant(tenant: string) {
    await attempt(async () => {
      await client.current?.close();
      client.current = undefined;
      setReady(false);
      await onTenant(tenant);
    });
  }
  async function restartStep() {
    await attempt(async () => {
      await client.current?.close();
      client.current = undefined;
      setReady(false);
      restart();
    });
  }
  return (
    <>
      {context.tenant_ids.length > 1 && !secondFactor && (
        <label className="field">
          Sign-in directory
          <select
            value={context.selected_tenant_id}
            onChange={(event) => void changeTenant(event.target.value)}
            disabled={busy || !ready}
          >
            {context.tenant_ids.map((tenant) => (
              <option value={tenant} key={tenant}>
                {tenant}
              </option>
            ))}
          </select>
        </label>
      )}
      {error && <Notice error>{error}</Notice>}
      {notice && <Notice>{notice}</Notice>}
      {secondFactor ? (
        <section>
          <h2>
            {secondFactor === "enroll"
              ? "Set up two-step verification"
              : "Two-step verification"}
          </h2>
          <p className="muted">
            {secondFactor === "enroll"
              ? "Add this key to your authenticator app, then enter the code it shows."
              : "Enter the code from your authenticator app to finish this proof."}
          </p>
          {enrollment && (
            <div className="setup-key">
              <span>Setup key</span>
              <code>{enrollment.secret}</code>
              {enrollment.uri.startsWith("otpauth://") && (
                <a href={enrollment.uri}>Open authenticator app</a>
              )}
            </div>
          )}
          <form onSubmit={verifyCode}>
            <label className="field">
              Verification code
              <input
                name="code"
                inputMode="numeric"
                autoComplete="one-time-code"
                pattern="[0-9]{6}"
                maxLength={6}
                required
              />
            </label>
            <button
              disabled={busy || (secondFactor === "enroll" && !enrollment)}
            >
              {busy ? "Verifying…" : "Verify code"}
            </button>
          </form>
          <button
            className="text-button"
            disabled={busy}
            onClick={() => void restartStep()}
          >
            Start this step again
          </button>
        </section>
      ) : (
        <>
          <div className="providers">
            {context.providers
              .filter((id) => id !== "password")
              .map((id) => (
                <button
                  className="secondary"
                  key={id}
                  onClick={() => void provider(id)}
                  disabled={!ready || busy}
                >
                  {id === "google.com"
                    ? "Continue with Google"
                    : `Continue with ${id}`}
                </button>
              ))}
          </div>
          {redirectProvider && (
            <button
              className="secondary"
              onClick={() =>
                void attempt(async () => {
                  await client.current!.redirect(redirectProvider);
                })
              }
              disabled={busy}
            >
              Continue in this tab
            </button>
          )}
          {context.providers.includes("password") && (
            <>
              <div className="divider">
                <span>or use email</span>
              </div>
              <form onSubmit={emailAction}>
                <label className="field">
                  Email
                  <input
                    type="email"
                    autoComplete="username"
                    value={email}
                    onChange={(event) => setEmail(event.target.value)}
                    required
                    disabled={busy}
                  />
                </label>
                <label className="field">
                  Password
                  <input
                    type="password"
                    autoComplete="current-password"
                    value={password}
                    onChange={(event) => setPassword(event.target.value)}
                    required
                    disabled={busy}
                  />
                </label>
                <button className="full-width" disabled={!ready || busy}>
                  {busy ? "Verifying…" : "Sign in with email"}
                </button>
              </form>
              <div className="email-tools">
                {context.password_sign_up && context.phase === "sign-in" && (
                  <button
                    className="text-button"
                    onClick={() => void register()}
                    disabled={!ready || busy || !email || !password}
                  >
                    Register a login
                  </button>
                )}
                <button
                  className="text-button"
                  onClick={() => void resend()}
                  disabled={!ready || busy || !email || !password}
                >
                  Resend verification email
                </button>
              </div>
            </>
          )}
          {!context.providers.length && (
            <Notice error>
              No sign-in methods are configured for this directory. Contact your
              administrator.
            </Notice>
          )}
          {!ready && error && (
            <button
              className="secondary"
              onClick={() => void restartStep()}
              disabled={busy}
            >
              Start this step again
            </button>
          )}
        </>
      )}
    </>
  );
}

function SignIn({ flow }: { flow: string }) {
  const loaded = useLoad<LoginContext>(
    `/login/context?flow=${encodeURIComponent(flow)}`,
  );
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<Error>();
  async function next(path: string, body: object) {
    const result = await request<Next>(path, {
      flow,
      csrf: loaded.data!.csrf,
      ...body,
    });
    if (result.redirect_to) navigate(result.redirect_to);
    else loaded.reload();
  }
  async function choose(choice: "new" | "existing") {
    setBusy(true);
    setError(undefined);
    try {
      await next("/login/choice", { choice });
    } catch (caught) {
      setError(caught as Error);
    } finally {
      setBusy(false);
    }
  }
  if (loaded.loading) return <Loading />;
  if (loaded.error)
    return <Failure error={loaded.error} retry={loaded.reload} />;
  const context = loaded.data!;
  return (
    <>
      <span className="eyebrow">Aidash Account · {context.client_id}</span>
      <h1>{titles[context.phase] ?? "Verify your sign-in"}</h1>
      <p className="intro">
        {descriptions[context.phase] ??
          "Complete a fresh authentication to continue."}
      </p>
      {error && <Failure error={error} />}
      {context.phase === "choose-account" ? (
        <div className="choices">
          {context.relink_only ? (
            <Notice>
              This sign-in method retains its original Aidash Account
              association. Prove that original Account to reattach it.
            </Notice>
          ) : (
            <button onClick={() => void choose("new")} disabled={busy}>
              Create a new Aidash Account
            </button>
          )}
          <button
            className="secondary"
            onClick={() => void choose("existing")}
            disabled={busy}
          >
            Use an existing Aidash Account
          </button>
          <p className="small muted">
            Accounts are never linked by matching email addresses. Existing
            Accounts are not merged.
          </p>
        </div>
      ) : (
        <SignInForm
          key={`${context.phase}:${context.selected_tenant_id}`}
          context={context}
          onProof={(token) => next("/login/verify", { id_token: token })}
          onTenant={async (tenant) => {
            await request("/login/tenant", {
              flow,
              csrf: context.csrf,
              tenant_id: tenant,
            });
            loaded.reload();
          }}
          restart={loaded.reload}
        />
      )}
    </>
  );
}

function AccountSettings() {
  const loaded = useLoad<Account>("/account");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<Error>();
  const [signedOut, setSignedOut] = useState(false);
  async function start(path: string, extra = {}) {
    setBusy(true);
    setError(undefined);
    try {
      const result = await request<Next>(path, {
        csrf: loaded.data!.csrf,
        ...extra,
      });
      if (!result.redirect_to)
        throw new Error("No verification flow was returned.");
      navigate(result.redirect_to);
    } catch (caught) {
      setError(caught as Error);
    } finally {
      setBusy(false);
    }
  }
  async function logoutAll() {
    setBusy(true);
    setError(undefined);
    try {
      const result = await request<{ completed: boolean }>("/logout/all", {
        csrf: loaded.data!.csrf,
      });
      if (!result.completed)
        throw new Error("Sign-out was not completed. Try again.");
      setSignedOut(true);
    } catch (caught) {
      setError(caught as Error);
    } finally {
      setBusy(false);
    }
  }
  if (signedOut)
    return (
      <>
        <span className="eyebrow">Aidash Account</span>
        <h1>Signed out everywhere</h1>
        <p className="intro">
          Sign in again from Cloud or Marketplace when you are ready. Work
          already admitted continues under its original authority.
        </p>
      </>
    );
  if (loaded.loading) return <Loading />;
  if (loaded.error)
    return (
      <>
        <h1>Your Aidash Account</h1>
        <Failure error={loaded.error} retry={loaded.reload} />
        <p className="small muted">
          If your session has ended, start sign-in from Cloud or Marketplace.
        </p>
      </>
    );
  const account = loaded.data!;
  const linked = account.identities.filter((identity) => identity.linked);
  return (
    <>
      <span className="eyebrow">Account settings</span>
      <h1>Sign-in methods</h1>
      <p className="intro">
        Changes require fresh proof. Linking does not combine product
        permissions or transfer ownership.
      </p>
      {error && <Failure error={error} />}
      <div className="account-reference">
        <span>Aidash Account</span>
        <code>{account.account_id}</code>
      </div>
      <div className="identity-list">
        {account.identities.map((identity) => (
          <article className="identity" key={identity.id}>
            <div className="identity-heading">
              <strong>{identity.linked ? "Linked" : "Unlinked"}</strong>
              {identity.id === account.source_identity_id && (
                <span className="badge">Current session</span>
              )}
            </div>
            <dl>
              <dt>Issuer</dt>
              <dd>{identity.issuer}</dd>
              <dt>Subject</dt>
              <dd>
                <code>{identity.subject}</code>
              </dd>
            </dl>
            {identity.linked && (
              <button
                className="secondary compact"
                disabled={busy || linked.length < 2}
                onClick={() =>
                  void start("/account/unlink/start", {
                    identity_id: identity.id,
                  })
                }
              >
                Unlink
              </button>
            )}
            {identity.linked && linked.length < 2 && (
              <p className="small muted">
                Keep at least one usable sign-in method.
              </p>
            )}
            {!identity.linked && (
              <p className="small muted">
                The original Account association is retained. Add this method
                again using fresh proof.
              </p>
            )}
          </article>
        ))}
      </div>
      <button disabled={busy} onClick={() => void start("/account/link/start")}>
        Add a sign-in method
      </button>
      <p className="small muted">
        Recover lost access through your identity provider. This page cannot
        replace its recovery checks.
      </p>
      <section className="global-logout">
        <h2>Sign out everywhere</h2>
        <p className="small muted">
          End sign-in sessions across all products and devices. Work already
          admitted continues.
        </p>
        <button
          className="secondary"
          disabled={busy}
          onClick={() => void logoutAll()}
        >
          Sign out of all products and devices
        </button>
      </section>
    </>
  );
}

function App() {
  if (location.pathname === "/settings/account")
    return (
      <Frame>
        <AccountSettings />
      </Frame>
    );
  const flow = new URLSearchParams(location.search).get("flow");
  return (
    <Frame>
      {flow ? (
        <SignIn flow={flow} />
      ) : (
        <>
          <span className="eyebrow">Aidash Account</span>
          <h1>Start with your product</h1>
          <p className="intro">
            Start sign-in from Cloud or Marketplace so we can safely return you
            to the right place.
          </p>
        </>
      )}
    </Frame>
  );
}
createRoot(document.getElementById("root")!).render(<App />);
