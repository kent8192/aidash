// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

export const EMAIL_KEY = "aidash.gcip.email";
// The email handoff is consumed exactly once. main.tsx takes it at startup,
// before /auth/config or the lazy sign-in chunk load, so a failure there
// cannot leave the address in sessionStorage. The module
// keeps the taken value so a development StrictMode re-render still sees it.
let takenEmailHint: string | undefined;
export function takeEmailHint() {
  if (takenEmailHint !== undefined) return takenEmailHint;
  try {
    takenEmailHint = sessionStorage.getItem(EMAIL_KEY) ?? "";
    sessionStorage.removeItem(EMAIL_KEY);
  } catch {
    takenEmailHint = "";
  }
  return takenEmailHint;
}
export function clearEmailHint() {
  try {
    sessionStorage.removeItem(EMAIL_KEY);
  } catch {
    // Storage unavailable; nothing to clear.
  }
}
