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
} from "firebase/auth";

export type GcipClientConfig = {
  project_id: string;
  api_key: string;
  auth_domain: string;
  tenant_id: string;
  providers: string[];
  password_sign_up: boolean;
};
export interface GcipClient {
  popup(id: string): Promise<string>;
  /** Navigates away; the sign-in completes in `redirectResult` on return. */
  redirect(id: string): Promise<never>;
  /** Null without network I/O unless this tab started a redirect. */
  redirectResult(): Promise<string | null>;
  password(email: string, password: string): Promise<string>;
  register(email: string, password: string): Promise<boolean>;
  resend(email: string, password: string): Promise<boolean>;
  close(): Promise<void>;
}
// Redirect sign-in resumes in a new page, so the app name must be stable.
const APP_NAME = "aidash-gcip";
function federatedProvider(id: string): AuthProvider {
  if (id === "google.com") {
    const provider = new GoogleAuthProvider();
    provider.setCustomParameters({ prompt: "select_account" });
    return provider;
  }
  return id.startsWith("saml.")
    ? new SAMLAuthProvider(id)
    : new OAuthProvider(id);
}
export function createClient(config: GcipClientConfig): GcipClient {
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
  return {
    async popup(id: string) {
      const result = await signInWithPopup(
        auth,
        federatedProvider(id),
        browserPopupRedirectResolver,
      );
      return result.user.getIdToken();
    },
    redirect(id: string): Promise<never> {
      return signInWithRedirect(
        auth,
        federatedProvider(id),
        browserPopupRedirectResolver,
      );
    },
    async redirectResult() {
      const result = await getRedirectResult(
        auth,
        browserPopupRedirectResolver,
      );
      return result ? result.user.getIdToken() : null;
    },
    async password(email: string, password: string) {
      const result = await signInWithEmailAndPassword(auth, email, password);
      return result.user.getIdToken();
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
      try {
        await signOut(auth);
      } finally {
        await deleteApp(app);
      }
    },
  };
}
