import { expect, test } from "bun:test";
import vectors from "../../../../audigma-settings/meta/condition-vectors.json";
import type { Condition, ConfigValue } from "../generated/types";
import { evalCondition } from "./condition";
import { diffLines, diffStats } from "./diff";
import { parseInline, parseMarkdown } from "./markdown";
import { pageFromHash } from "./route";

test("condition evaluator passes the shared vectors", () => {
  expect(vectors.length).toBeGreaterThanOrEqual(10);
  for (const v of vectors as unknown as {
    name: string;
    condition: Condition;
    values: Record<string, ConfigValue>;
    expected: boolean;
  }[]) {
    const got = evalCondition(v.condition, (p) => v.values[p]);
    expect(got, v.name).toBe(v.expected);
  }
});

test("markdown: paragraphs, bullets, code and emphasis", () => {
  const blocks = parseMarkdown("First *line*\nsecond `x`.\n\n- one\n- two\n  more\n\nLast");
  expect(blocks).toEqual([
    {
      kind: "p",
      inline: [
        { kind: "text", text: "First " },
        { kind: "em", text: "line" },
        { kind: "text", text: " second " },
        { kind: "code", text: "x" },
        { kind: "text", text: "." },
      ],
    },
    {
      kind: "ul",
      items: [
        [{ kind: "text", text: "one" }],
        [
          { kind: "text", text: "two" },
          { kind: "text", text: " more" },
        ],
      ],
    },
    { kind: "p", inline: [{ kind: "text", text: "Last" }] },
  ]);
});

test("markdown: unmatched markers stay literal", () => {
  expect(parseInline("a * b `c")).toEqual([{ kind: "text", text: "a * b `c" }]);
});

test("diff: identical, add, delete, replace", () => {
  expect(diffStats(diffLines("a\nb\n", "a\nb\n"))).toEqual({ added: 0, removed: 0 });
  expect(diffLines("a\nc\n", "a\nb\nc\n")).toEqual([
    { kind: "same", text: "a" },
    { kind: "add", text: "b" },
    { kind: "same", text: "c" },
  ]);
  expect(diffLines("a\nb\nc", "a\nc")).toEqual([
    { kind: "same", text: "a" },
    { kind: "del", text: "b" },
    { kind: "same", text: "c" },
  ]);
  expect(diffLines("x\ny\n", "x\nz\n")).toEqual([
    { kind: "same", text: "x" },
    { kind: "del", text: "y" },
    { kind: "add", text: "z" },
  ]);
  expect(diffLines("", "a\n")).toEqual([{ kind: "add", text: "a" }]);
});

test("diff: reordered blocks use the longest common subsequence", () => {
  const s = diffStats(diffLines("a\nb\nc\nd\n", "b\nc\nd\na\n"));
  expect(s).toEqual({ added: 1, removed: 1 });
});

test("route: hash names a page", () => {
  expect(pageFromHash("#/review")).toBe("review");
  expect(pageFromHash("#form")).toBe("form");
  expect(pageFromHash("")).toBeNull();
  expect(pageFromHash("#/nope")).toBeNull();
});
