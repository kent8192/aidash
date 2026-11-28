import { sha256 } from "@noble/hashes/sha2.js";

const CHUNK_BYTES = 4 * 1024 * 1024;

export async function hashUpload(file: Blob): Promise<string> {
  const state = sha256.create();
  for (let offset = 0; offset < file.size; offset += CHUNK_BYTES) {
    const chunk = await file.slice(offset, offset + CHUNK_BYTES).arrayBuffer();
    state.update(new Uint8Array(chunk));
  }
  return Array.from(state.digest(), (byte) =>
    byte.toString(16).padStart(2, "0"),
  ).join("");
}

export async function uploadChunks(
  file: Blob,
  uploadedBytes: number,
  send: (chunk: { offset: number; data: string }) => Promise<{
    uploaded_bytes: number;
  }>,
  progress?: (bytes: number) => void,
) {
  let offset = uploadedBytes;
  while (true) {
    if (
      !Number.isSafeInteger(offset) ||
      offset < 0 ||
      offset > file.size ||
      (offset !== file.size && offset % CHUNK_BYTES !== 0)
    )
      throw new Error("Invalid upload continuation");
    if (offset === file.size) return;
    const chunk = await file.slice(offset, offset + CHUNK_BYTES).arrayBuffer();
    let binary = "";
    for (const byte of new Uint8Array(chunk))
      binary += String.fromCharCode(byte);
    const receipt = await send({ offset, data: btoa(binary) });
    if (receipt.uploaded_bytes < offset + chunk.byteLength)
      throw new Error("Upload receipt did not advance");
    offset = receipt.uploaded_bytes;
    progress?.(offset);
  }
}
