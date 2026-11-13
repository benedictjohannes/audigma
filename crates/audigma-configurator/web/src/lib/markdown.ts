/**
 * The markdown subset of the metadata (plan §5): paragraphs (blank line), `- ` bullet lists,
 * `` `code` `` spans and `*emphasis*`. Nothing else.
 */
export type Inline = { kind: "text" | "code" | "em"; text: string };
export type Block = { kind: "p"; inline: Inline[] } | { kind: "ul"; items: Inline[][] };

export function parseInline(src: string): Inline[] {
  const out: Inline[] = [];
  let text = "";
  const flush = () => {
    if (text) out.push({ kind: "text", text });
    text = "";
  };
  let i = 0;
  while (i < src.length) {
    const ch = src[i];
    if (ch === "`" || ch === "*") {
      const end = src.indexOf(ch, i + 1);
      if (end > i + 1) {
        flush();
        out.push({ kind: ch === "`" ? "code" : "em", text: src.slice(i + 1, end) });
        i = end + 1;
        continue;
      }
    }
    text += ch;
    i++;
  }
  flush();
  return out;
}

export function parseMarkdown(src: string): Block[] {
  const blocks: Block[] = [];
  let para: string[] = [];
  let list: Inline[][] | null = null;
  const endPara = () => {
    if (para.length) blocks.push({ kind: "p", inline: parseInline(para.join(" ")) });
    para = [];
  };
  const endList = () => {
    if (list) blocks.push({ kind: "ul", items: list });
    list = null;
  };
  for (const raw of src.split("\n")) {
    const line = raw.trim();
    if (line === "") {
      endPara();
      endList();
    } else if (raw.startsWith("- ")) {
      endPara();
      list ??= [];
      list.push(parseInline(raw.slice(2).trim()));
    } else if (list && /^\s+\S/.test(raw)) {
      // continuation of the previous bullet
      list[list.length - 1].push(...parseInline(` ${line}`));
    } else {
      endList();
      para.push(line);
    }
  }
  endPara();
  endList();
  return blocks;
}
