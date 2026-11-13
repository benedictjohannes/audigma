import { keyed } from "../lib/keys";
import { type Block, type Inline, parseMarkdown } from "../lib/markdown";

function Spans({ inline }: { inline: Inline[] }) {
  return keyed(inline, (s) => `${s.kind}:${s.text}`).map(({ key, item: s }) =>
    s.kind === "code" ? (
      <code key={key} className="rounded-sm bg-code px-1 font-mono text-[0.92em]">
        {s.text}
      </code>
    ) : s.kind === "em" ? (
      <em key={key}>{s.text}</em>
    ) : (
      <span key={key}>{s.text}</span>
    ),
  );
}

const inlineText = (inline: Inline[]) => inline.map((s) => s.text).join("");
const blockText = (b: Block) =>
  b.kind === "p" ? `p:${inlineText(b.inline)}` : `ul:${b.items.map(inlineText).join("|")}`;

/** Renders the metadata markdown subset (plan §5). */
export function Markdown({ text }: { text: string }) {
  return (
    <div className="space-y-2">
      {keyed(parseMarkdown(text), blockText).map(({ key, item: b }) =>
        b.kind === "p" ? (
          <p key={key}>
            <Spans inline={b.inline} />
          </p>
        ) : (
          <ul key={key} className="list-disc space-y-1 pl-5">
            {keyed(b.items, inlineText).map(({ key: k, item: it }) => (
              <li key={k}>
                <Spans inline={it} />
              </li>
            ))}
          </ul>
        ),
      )}
    </div>
  );
}
