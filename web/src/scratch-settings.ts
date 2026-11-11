/** Convert display units to the integer units accepted by the API. */
export const bytesFromGiB = (value: string): number =>
  Math.round(Number(value) * 1024 ** 3);

export const millisFromCores = (value: string): number =>
  Math.round(Number(value) * 1000);

/** Include the linked space even when it falls outside the recent list. */
export async function loadScratchSpaces<T extends { id: string; node: string }>(
  node: string,
  focus: string | undefined,
  list: () => Promise<T[]>,
  read: (id: string) => Promise<T>,
): Promise<{ spaces: T[]; unavailable: boolean }> {
  const spaces = await list();
  if (!focus || spaces.some((space) => space.id === focus))
    return { spaces, unavailable: false };
  try {
    const space = await read(focus);
    if (space.id === focus && space.node === node)
      return { spaces: [...spaces, space], unavailable: false };
  } catch {
    // A deleted, inaccessible, or temporarily unavailable space cannot be shown.
  }
  return { spaces, unavailable: true };
}
