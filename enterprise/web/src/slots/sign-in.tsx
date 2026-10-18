import { lazy } from "react";
import { takeEmailHint } from "../gcip-email-hint";
import type { SignInMethod } from "@/slots/types";

// Move any discovery email handoff out of sessionStorage as the page loads,
// before /auth/config or the lazy sign-in chunk, so a failure there cannot
// leave the address behind.
takeEmailHint();

export const signInMethods: Readonly<Record<string, SignInMethod>> = {
  gcip: {
    Page: lazy(() => import("../gcip-sign-in")),
    label: { "ja-JP": "サインイン", "en-US": "Sign in" },
  },
};
