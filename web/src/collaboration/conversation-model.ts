/** Pages arrive newest-first; each server page is chronological. */
export function mergeMessagePages<
  T extends { message: { id: string; created_at: string } },
>(pages: readonly { messages: readonly T[] }[]): T[] {
  const messages = new Map<string, T>();
  for (const page of [...pages].reverse()) {
    for (const item of page.messages) messages.set(item.message.id, item);
  }
  return [...messages.values()].sort(
    (a, b) =>
      a.message.created_at.localeCompare(b.message.created_at) ||
      a.message.id.localeCompare(b.message.id),
  );
}

export type MessageSubmission = {
  workspace: string;
  thread: string | null;
  content: string;
  key: string;
  attachments: string[];
};

/** Retries retain identity only while channel, thread and payload agree. */
export function submissionFor(
  previous: MessageSubmission | null,
  workspace: string,
  thread: string | null,
  draft: string,
  newKey: () => string,
  attachments: readonly string[] = [],
): MessageSubmission {
  const content = draft.trim();
  const canonical = [...attachments].sort();
  if (
    previous?.workspace === workspace &&
    previous.thread === thread &&
    previous.content === content &&
    JSON.stringify(previous.attachments) === JSON.stringify(canonical)
  ) {
    return previous;
  }
  return { workspace, thread, content, key: newKey(), attachments: canonical };
}

export const MAX_ATTACHMENT_BYTES = 1024 * 1024;
export const MAX_ATTACHMENTS = 8;
export function validAttachments(
  files: readonly { name: string; size: number }[],
): boolean {
  return (
    files.length <= MAX_ATTACHMENTS &&
    files.every(
      (file) =>
        file.name.length > 0 &&
        file.size > 0 &&
        file.size <= MAX_ATTACHMENT_BYTES &&
        new TextEncoder().encode(file.name).length <= 255 &&
        file.name !== "." &&
        file.name !== ".." &&
        !/[\\/]/.test(file.name) &&
        !Array.from(file.name).some(
          (character) =>
            character.charCodeAt(0) < 32 || character.charCodeAt(0) === 127,
        ),
    )
  );
}
