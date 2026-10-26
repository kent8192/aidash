// SPDX-FileCopyrightText: 2026 kent8192
// SPDX-License-Identifier: BUSL-1.1
// See enterprise/LICENSE.

// Account settings starts a normal GCIP sign-in with the intent to enroll a
// second factor. The intent must survive the provider's redirect round trip, so
// it lives in sessionStorage with a lifetime of one login transaction (10 min).
const KEY = "aidash.gcip.enroll";
const LIFETIME_MS = 10 * 60 * 1000;
export function markEnrollIntent() {
  try {
    sessionStorage.setItem(KEY, String(Date.now()));
  } catch {
    // Without storage the user simply signs in; they can retry from settings.
  }
}
export function hasEnrollIntent() {
  try {
    const marked = Number(sessionStorage.getItem(KEY));
    return marked > 0 && Date.now() - marked < LIFETIME_MS;
  } catch {
    return false;
  }
}
export function clearEnrollIntent() {
  try {
    sessionStorage.removeItem(KEY);
  } catch {
    // Storage unavailable; nothing to clear.
  }
}
