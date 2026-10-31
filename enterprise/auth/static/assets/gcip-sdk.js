// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

// GCIP is loaded only after the Authentication Server supplies a login context.
import { initializeApp, deleteApp } from "./vendor/firebase-app.js";
import {
  initializeAuth,
  inMemoryPersistence,
  browserPopupRedirectResolver,
  signInWithPopup,
  signInWithRedirect,
  getRedirectResult,
  GoogleAuthProvider,
  OAuthProvider,
  SAMLAuthProvider,
  signInWithEmailAndPassword,
  createUserWithEmailAndPassword,
  sendEmailVerification,
  signOut,
  getMultiFactorResolver,
  multiFactor,
  TotpMultiFactorGenerator,
} from "./vendor/firebase-auth.js";
import {
  ENROLLMENT_REQUIRED,
  SECOND_FACTOR_REQUIRED,
  SECOND_FACTOR_UNSUPPORTED,
} from "./gcip-second-factor.js";
// Redirect sign-in resumes in a new page, so the app name must be stable.
const APP_NAME = "aidash-gcip";
function federatedProvider(id, loginHint) {
  const hint = loginHint ? { login_hint: loginHint } : {};
  if (id === "google.com") {
    const provider = new GoogleAuthProvider();
    provider.setCustomParameters({ prompt: "select_account", ...hint });
    return provider;
  }
  const provider = id.startsWith("saml.")
    ? new SAMLAuthProvider(id)
    : new OAuthProvider(id);
  // SAML providers may not accept custom parameters; set only when supported.
  if (loginHint && typeof provider.setCustomParameters === "function")
    provider.setCustomParameters(hint);
  return provider;
}
function coded(code) {
  return Object.assign(new Error(code), { code });
}
function codeOf(error) {
  return typeof error === "object" && error !== null && "code" in error
    ? error.code
    : undefined;
}
export function createClient(config, options = {}) {
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
  let challenge;
  let enrolling;
  const complete = async (user) => {
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
  const settle = async (attempt) => {
    try {
      const result = await attempt;
      return result ? await complete(result.user) : null;
    } catch (error) {
      if (codeOf(error) !== "auth/multi-factor-auth-required") throw error;
      challenge = getMultiFactorResolver(auth, error);
      throw coded(SECOND_FACTOR_REQUIRED);
    }
  };
  const settled = async (attempt) => await settle(attempt);
  return {
    popup(id, loginHint) {
      return settled(
        signInWithPopup(
          auth,
          federatedProvider(id, loginHint),
          browserPopupRedirectResolver,
        ),
      );
    },
    redirect(id, loginHint) {
      return signInWithRedirect(
        auth,
        federatedProvider(id, loginHint),
        browserPopupRedirectResolver,
      );
    },
    redirectResult() {
      return settle(getRedirectResult(auth, browserPopupRedirectResolver));
    },
    password(email, password) {
      return settled(signInWithEmailAndPassword(auth, email, password));
    },
    async resolveSecondFactor(code) {
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
      // Settings asked to set up an authenticator: a user who already holds a
      // factor proved it here, and now adds the new one before signing in.
      if (options.enroll) {
        enrolling = { user: result.user };
        throw coded(ENROLLMENT_REQUIRED);
      }
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
    async finishEnrollment(code) {
      if (!enrolling?.secret) throw coded("auth/missing-multi-factor-session");
      const { user, secret } = enrolling;
      await multiFactor(user).enroll(
        TotpMultiFactorGenerator.assertionForEnrollment(secret, code),
        "Authenticator app",
      );
      enrolling = undefined;
      return user.getIdToken(true);
    },
    async register(email, password) {
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
    async resend(email, password) {
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
