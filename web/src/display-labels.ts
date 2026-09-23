// Distinguish equal display names within one authorized snapshot without showing IDs.
export function disambiguateLabels<T>(
  items: readonly T[],
  key: (item: T) => string,
  label: (item: T) => string,
  scope: (item: T) => string = () => "",
): Map<string, string> {
  const groups = new Map<string, T[]>();
  for (const item of items) {
    const group = `${scope(item)}\0${label(item)}`;
    groups.set(group, [...(groups.get(group) ?? []), item]);
  }
  const result = new Map<string, string>();
  for (const group of groups.values()) {
    group.sort((a, b) => (key(a) < key(b) ? -1 : key(a) > key(b) ? 1 : 0));
    group.forEach((item, index) => {
      result.set(
        key(item),
        `${label(item)}${group.length > 1 ? ` (#${index + 1})` : ""}`,
      );
    });
  }
  return result;
}
