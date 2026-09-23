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
};

/** Retries retain identity only while channel, thread and payload agree. */
export function submissionFor(
  previous: MessageSubmission | null,
  workspace: string,
  thread: string | null,
  draft: string,
  newKey: () => string,
): MessageSubmission {
  const content = draft.trim();
  if (
    previous?.workspace === workspace &&
    previous.thread === thread &&
    previous.content === content
  ) {
    return previous;
  }
  return { workspace, thread, content, key: newKey() };
}
