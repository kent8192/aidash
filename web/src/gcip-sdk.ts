// The npm SDK stays behind the lazy sign-in route. Tests replace this module.
import { initializeApp, deleteApp } from "firebase/app";
import {
  initializeAuth,
  inMemoryPersistence,
  browserPopupRedirectResolver,
  signInWithPopup,
  signInWithRedirect,
  getRedirectResult,
  GoogleAuthProvider,
  type AuthProvider,
  OAuthProvider,
  SAMLAuthProvider,
  signInWithEmailAndPassword,
  createUserWithEmailAndPassword,
  sendEmailVerification,
  signOut,
  getMultiFactorResolver,
  multiFactor,
  TotpMultiFactorGenerator,
  type MultiFactorError,
  type MultiFactorResolver,
  type TotpSecret,
  type User,
  type UserCredential,
} from "firebase/auth";
import {
  ENROLLMENT_REQUIRED,
  SECOND_FACTOR_REQUIRED,
  SECOND_FACTOR_UNSUPPORTED,
} from "./gcip-second-factor";

export type GcipClientConfig = {
  project_id: string;
  api_key: string;
  auth_domain: string;
  tenant_id: string;
  providers: string[];
  password_sign_up: boolean;
  /** A hint only: GCIP enforces the MFA Requirement, Aidash never reads its claims. */
  mfa_mandatory: boolean;
};
export type GcipClientOptions = {
  /** Lead a user with no second factor through TOTP enrollment (account settings). */
  enroll?: boolean;
};
export type TotpEnrollment = { secret: string; uri: string };
export interface GcipClient {
  /**
   * The first-factor methods resolve with the ID token only when no second
   * factor is outstanding. Otherwise they reject with SECOND_FACTOR_REQUIRED or
   * ENROLLMENT_REQUIRED and this client keeps the pending sign-in, so the page
   * must finish it with the methods below before `close`.
   */
  popup(id: string, loginHint?: string): Promise<string>;
  /** Navigates away; the sign-in completes in `redirectResult` on return. */
  redirect(id: string, loginHint?: string): Promise<never>;
  /** Null without network I/O unless this tab started a redirect. */
  redirectResult(): Promise<string | null>;
  password(email: string, password: string): Promise<string>;
  /** Answers a pending SECOND_FACTOR_REQUIRED challenge with a TOTP code. */
  resolveSecondFactor(code: string): Promise<string>;
  /** Starts TOTP enrollment for a pending ENROLLMENT_REQUIRED sign-in. */
  startEnrollment(): Promise<TotpEnrollment>;
  /** Verifies the first code, enrolls the factor and returns a fresh ID token. */
  finishEnrollment(code: string): Promise<string>;
  register(email: string, password: string): Promise<boolean>;
  resend(email: string, password: string): Promise<boolean>;
  close(): Promise<void>;
}
// Redirect sign-in resumes in a new page, so the app name must be stable.
const APP_NAME = "aidash-gcip";
function federatedProvider(id: string, loginHint?: string): AuthProvider {
  const hint: Record<string, string> = loginHint
    ? { login_hint: loginHint }
    : {};
  if (id === "google.com") {
    const provider = new GoogleAuthProvider();
    provider.setCustomParameters({ prompt: "select_account", ...hint });
    return provider;
  }
  const provider = id.startsWith("saml.")
    ? new SAMLAuthProvider(id)
    : new OAuthProvider(id);
  // SAML providers may not accept custom parameters; set only when supported.
  if (
    loginHint &&
    typeof (provider as Partial<OAuthProvider>).setCustomParameters ===
      "function"
  )
    provider.setCustomParameters(hint);
  return provider;
}
function coded(code: string) {
  return Object.assign(new Error(code), { code });
}
function codeOf(error: unknown) {
  return typeof error === "object" && error !== null && "code" in error
    ? error.code
    : undefined;
}
export function createClient(
  config: GcipClientConfig,
  options: GcipClientOptions = {},
): GcipClient {
  const app = initializeApp(
    {
      projectId: config.project_id,
      apiKey: config.api_key,
      authDomain: config.auth_domain,
    },
    APP_NAME,
  );
  const auth = initializeAuth(app, { persistence: inMemoryPersistence });
  auth.tenantId = config.tenant_id;
  let closed = false;
  // The sign-in a second factor is still owed on; at most one is pending.
  let challenge: MultiFactorResolver | undefined;
  let enrolling: { user: User; secret?: TotpSecret } | undefined;
  const complete = async (user: User) => {
    // GCIP challenges every user who holds a factor, so a user with none is
    // here only when the tenant is mandatory or settings asked to enroll.
    if (
      multiFactor(user).enrolledFactors.length === 0 &&
      (config.mfa_mandatory || options.enroll)
    ) {
      enrolling = { user };
      throw coded(ENROLLMENT_REQUIRED);
    }
    return user.getIdToken();
  };
  const settle = async (attempt: Promise<UserCredential | null>) => {
    try {
      const result = await attempt;
      return result ? await complete(result.user) : null;
    } catch (error) {
      if (codeOf(error) !== "auth/multi-factor-auth-required") throw error;
      challenge = getMultiFactorResolver(auth, error as MultiFactorError);
      throw coded(SECOND_FACTOR_REQUIRED);
    }
  };
  const settled = async (attempt: Promise<UserCredential>) =>
    (await settle(attempt)) as string;
  return {
    popup(id: string, loginHint?: string) {
      return settled(
        signInWithPopup(
          auth,
          federatedProvider(id, loginHint),
          browserPopupRedirectResolver,
        ),
      );
    },
    redirect(id: string, loginHint?: string): Promise<never> {
      return signInWithRedirect(
        auth,
        federatedProvider(id, loginHint),
        browserPopupRedirectResolver,
      );
    },
    redirectResult() {
      return settle(getRedirectResult(auth, browserPopupRedirectResolver));
    },
    password(email: string, password: string) {
      return settled(signInWithEmailAndPassword(auth, email, password));
    },
    async resolveSecondFactor(code: string) {
      const hint = challenge?.hints.find(
        (candidate) =>
          candidate.factorId === TotpMultiFactorGenerator.FACTOR_ID,
      );
      if (!challenge) throw coded("auth/missing-multi-factor-session");
      if (!hint) throw coded(SECOND_FACTOR_UNSUPPORTED);
      const result = await challenge.resolveSignIn(
        TotpMultiFactorGenerator.assertionForSignIn(hint.uid, code),
      );
      challenge = undefined;
      return result.user.getIdToken();
    },
    async startEnrollment() {
      if (!enrolling) throw coded("auth/missing-multi-factor-session");
      const { user } = enrolling;
      const session = await multiFactor(user).getSession();
      enrolling.secret = await TotpMultiFactorGenerator.generateSecret(session);
      return {
        secret: enrolling.secret.secretKey,
        uri: enrolling.secret.generateQrCodeUrl(
          user.email ?? undefined,
          "Aidash",
        ),
      };
    },
    async finishEnrollment(code: string) {
      if (!enrolling?.secret) throw coded("auth/missing-multi-factor-session");
      const { user, secret } = enrolling;
      await multiFactor(user).enroll(
        TotpMultiFactorGenerator.assertionForEnrollment(secret, code),
        "Authenticator app",
      );
      enrolling = undefined;
      return user.getIdToken(true);
    },
    async register(email: string, password: string) {
      const result = await createUserWithEmailAndPassword(
        auth,
        email,
        password,
      );
      try {
        await sendEmailVerification(result.user);
        return true;
      } catch {
        // Creation succeeded. Recovery must authenticate the existing user
        // rather than attempting to create the same account again.
        return false;
      } finally {
        await signOut(auth);
      }
    },
    async resend(email: string, password: string) {
      const result = await signInWithEmailAndPassword(auth, email, password);
      try {
        if (result.user.emailVerified) return false;
        await sendEmailVerification(result.user);
        return true;
      } finally {
        await signOut(auth);
      }
    },
    async close() {
      if (closed) return;
      closed = true;
      challenge = enrolling = undefined;
      try {
        await signOut(auth);
      } finally {
        await deleteApp(app);
      }
    },
  };
}
