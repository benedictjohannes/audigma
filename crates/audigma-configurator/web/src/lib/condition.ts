import type { Condition, ConfigValue } from "../generated/types";

/** Resolved config value of an absolute dotted path, after default resolution; `undefined` when unresolved. */
export type Resolve = (path: string) => ConfigValue | undefined;

/** Numbers compare by value (`6` equals `6.0`); everything else strictly. */
function same(a: ConfigValue, b: ConfigValue): boolean {
  return a === b;
}

/**
 * Evaluates a structured condition (Decision 14). Mirrors `Condition::eval` in Rust: an unresolved
 * key makes `eq` and `in` false and `neq` true. Must pass `condition-vectors.json`.
 */
export function evalCondition(c: Condition, resolve: Resolve): boolean {
  if ("all" in c) return c.all.every((x) => evalCondition(x, resolve));
  if ("any" in c) return c.any.some((x) => evalCondition(x, resolve));
  const v = resolve(c.key);
  if ("eq" in c) return v !== undefined && same(v, c.eq);
  if ("neq" in c) return !(v !== undefined && same(v, c.neq));
  return v !== undefined && c.in.some((x) => same(v, x));
}
