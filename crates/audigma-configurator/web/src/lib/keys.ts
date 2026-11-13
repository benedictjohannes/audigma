/**
 * Stable React keys for a list that has no ids: the item's own text, plus a counter so that
 * repeated texts stay distinct. For a list that is re-rendered whole this identifies an item by
 * what it says rather than where it sits.
 */
export function keyed<T>(
  items: readonly T[],
  text: (item: T) => string,
): { key: string; item: T }[] {
  const seen = new Map<string, number>();
  return items.map((item) => {
    const t = text(item);
    const n = seen.get(t) ?? 0;
    seen.set(t, n + 1);
    return { key: `${t}#${n}`, item };
  });
}
