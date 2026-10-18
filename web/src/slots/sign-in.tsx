import type { SignInMethod } from "./types";

/** Community signs in through the OIDC redirect alone. */
export const signInMethods: Readonly<Record<string, SignInMethod>> = {};
