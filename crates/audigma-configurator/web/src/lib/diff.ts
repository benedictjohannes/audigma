export type DiffKind = "same" | "add" | "del";
export interface DiffLine {
  kind: DiffKind;
  text: string;
}

function lines(s: string): string[] {
  if (s === "") return [];
  const out = s.split("\n");
  if (out[out.length - 1] === "") out.pop();
  return out;
}

/**
 * Line diff of `before` against `after` (longest common subsequence, after trimming the common
 * head and tail). Config files are small, so the quadratic table on the differing middle is fine.
 */
export function diffLines(before: string, after: string): DiffLine[] {
  const a = lines(before);
  const b = lines(after);
  let head = 0;
  while (head < a.length && head < b.length && a[head] === b[head]) head++;
  let tail = 0;
  while (
    tail < a.length - head &&
    tail < b.length - head &&
    a[a.length - 1 - tail] === b[b.length - 1 - tail]
  )
    tail++;
  const am = a.slice(head, a.length - tail);
  const bm = b.slice(head, b.length - tail);
  const w = bm.length + 1;
  const t = new Uint32Array((am.length + 1) * w);
  for (let i = am.length - 1; i >= 0; i--)
    for (let j = bm.length - 1; j >= 0; j--)
      t[i * w + j] =
        am[i] === bm[j]
          ? t[(i + 1) * w + j + 1] + 1
          : Math.max(t[(i + 1) * w + j], t[i * w + j + 1]);
  const out: DiffLine[] = a.slice(0, head).map((text) => ({ kind: "same", text }));
  let i = 0;
  let j = 0;
  while (i < am.length && j < bm.length) {
    if (am[i] === bm[j]) {
      out.push({ kind: "same", text: am[i] });
      i++;
      j++;
    } else if (t[(i + 1) * w + j] >= t[i * w + j + 1]) {
      out.push({ kind: "del", text: am[i++] });
    } else {
      out.push({ kind: "add", text: bm[j++] });
    }
  }
  while (i < am.length) out.push({ kind: "del", text: am[i++] });
  while (j < bm.length) out.push({ kind: "add", text: bm[j++] });
  for (const text of a.slice(a.length - tail)) out.push({ kind: "same", text });
  return out;
}

/** Counts of added and removed lines. */
export function diffStats(d: DiffLine[]): { added: number; removed: number } {
  return {
    added: d.filter((l) => l.kind === "add").length,
    removed: d.filter((l) => l.kind === "del").length,
  };
}
