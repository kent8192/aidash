// Error codes shared by the lazy GCIP SDK adapter and the sign-in page. They
// live outside the adapter so tests can replace it without redefining them.
/** `code` of the error a first-factor method rejects with while a TOTP code is due. */
export const SECOND_FACTOR_REQUIRED = "aidash/second-factor-required";
/** `code` of the error a first-factor method rejects with while a factor must be enrolled. */
export const ENROLLMENT_REQUIRED = "aidash/enrollment-required";
/** `code` of the error raised when the user holds only factors this client cannot answer. */
export const SECOND_FACTOR_UNSUPPORTED = "aidash/second-factor-unsupported";
