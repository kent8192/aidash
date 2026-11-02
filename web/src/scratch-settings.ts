/** Convert display units to the integer units accepted by the API. */
export const bytesFromGiB = (value: string): number =>
  Math.round(Number(value) * 1024 ** 3);

export const millisFromCores = (value: string): number =>
  Math.round(Number(value) * 1000);
