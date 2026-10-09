// The npm SDK stays behind the lazy sign-in route. Tests replace this module.
import { initializeApp, deleteApp } from "firebase/app";
import {
  initializeAuth,
  inMemoryPersistence,
  browserPopupRedirectResolver,
  signInWithPopup,
  GoogleAuthProvider,
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
export function createClient(config: GcipClientConfig) {
  const app = initializeApp(
    {
      projectId: config.project_id,
      apiKey: config.api_key,
      authDomain: config.auth_domain,
    },
    `aidash-${crypto.randomUUID()}`,
  );
  const auth = initializeAuth(app, { persistence: inMemoryPersistence });
  auth.tenantId = config.tenant_id;
  let closed = false;
  return {
    async popup(id: string) {
      const provider =
        id === "google.com"
          ? new GoogleAuthProvider()
          : id.startsWith("saml.")
            ? new SAMLAuthProvider(id)
            : new OAuthProvider(id);
      if (id === "google.com")
        provider.setCustomParameters({ prompt: "select_account" });
      const result = await signInWithPopup(
        auth,
        provider,
        browserPopupRedirectResolver,
      );
      return result.user.getIdToken();
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
