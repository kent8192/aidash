export type Utf8DecodeResult = { ok: true; text: string } | { ok: false };

/** True when the bytes start with a UTF-16 byte-order mark. */
export function hasUtf16Bom(bytes: ArrayBuffer): boolean {
  if (bytes.byteLength < 2) return false;
  const head = new Uint8Array(bytes, 0, 2);
  return (
    (head[0] === 0xfe && head[1] === 0xff) ||
    (head[0] === 0xff && head[1] === 0xfe)
  );
}

/**
 * Decodes strict UTF-8 and strips a leading UTF-8 BOM. Invalid bytes are
 * refused instead of replaced with U+FFFD, so a non-UTF-8 CSV is reported as
 * such rather than rendered as garbage.
 */
export function decodeUtf8(bytes: ArrayBuffer): Utf8DecodeResult {
  try {
    const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    return { ok: true, text: text.replace(/^\uFEFF/, "") };
  } catch {
    return { ok: false };
  }
}
