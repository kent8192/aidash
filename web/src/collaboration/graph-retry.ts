/** Retry only a fresh projection; stale continuations must restart at the caller. */
export async function retryFreshGraphPage<T>(
  cursor: string | null,
  load: () => Promise<T>,
): Promise<T> {
  for (let attempt = 0; ; attempt++) {
    try {
      return await load();
    } catch (error) {
      if (
        cursor !== null ||
        attempt >= 2 ||
        !(error instanceof Error && "status" in error && error.status === 409)
      )
        throw error;
    }
  }
}
