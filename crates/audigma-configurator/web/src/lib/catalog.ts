import type {
  Assist,
  Command,
  ConfigScalar,
  ConfigValue,
  Field,
  MetaBundle,
  OptionMeta,
  SectionBundle,
  UseCase,
  ValueTree,
} from "../generated/types";
import { evalCondition } from "./condition";

/** Which commands the form shows keys for (mirrors `Mode` in the TUI). */
export type Mode = "all" | "live" | "offline";

const MODE_COMMANDS: Record<Mode, Command[]> = {
  all: ["run", "play", "convert"],
  live: ["run", "play"],
  offline: ["convert"],
};

export const USE_CASES: { id: UseCase; label: string; blurb: string }[] = [
  {
    id: "interactive-desktop",
    label: "Interactive desktop",
    blurb: "A daemon that stays ready on a desktop; apps, browsers and MPD play into it.",
  },
  {
    id: "headless-appliance",
    label: "Headless appliance",
    blurb: "A dedicated music box with no desktop sound server.",
  },
  {
    id: "offline-conversion",
    label: "Offline conversion",
    blurb: "Converting files with `audigma convert`.",
  },
];

export function labelOfUseCase(u: UseCase): string {
  return USE_CASES.find((x) => x.id === u)?.label ?? u;
}

/** The mode a use case starts the form in. */
export function modeForUseCase(u: UseCase | null): Mode {
  if (u === null) return "all";
  return u === "offline-conversion" ? "offline" : "live";
}

export type Values = Record<string, ConfigValue>;

/** Dotted path -> value for every leaf of the draft's nested value tree. */
export function flatten(tree: ValueTree, prefix = "", out: Values = {}): Values {
  for (const [k, v] of Object.entries(tree)) {
    const path = prefix ? `${prefix}.${k}` : k;
    if (v !== null && typeof v === "object" && !Array.isArray(v))
      flatten(v as ValueTree, path, out);
    else out[path] = v as ConfigValue;
  }
  return out;
}

/** The text an option key is compared against: strings as is, numbers and booleans by their text. */
export function optionText(v: ConfigValue | undefined): string | null {
  if (v === undefined || Array.isArray(v)) return null;
  return String(v);
}

/** The value of option `key`, typed like the metadata (numbers and booleans match by their text). */
export function optionValue(key: string): ConfigScalar {
  if (/^-?\d+$/.test(key)) return Number(key);
  if (key === "true") return true;
  if (key === "false") return false;
  return key;
}

/** `apodization = "gentle"` is stored as its 19 kHz edge; compare options through that alias. */
function canon(path: string, t: string | null): string | null {
  return path === "resampler.apodization" && t === "gentle" ? "19000" : t;
}

export function sameOption(
  path: string,
  a: ConfigValue | undefined,
  b: ConfigValue | undefined,
): boolean {
  const x = canon(path, optionText(a));
  const y = canon(path, optionText(b));
  return x !== null && y !== null && x === y;
}

export function isBoolField(f: Field): boolean {
  return (
    typeof f.default === "boolean" ||
    Object.values(f.recommended).some((r) => typeof r?.value === "boolean") ||
    /^(true|false)/.test(f.default_text ?? "")
  );
}

export function showValue(v: ConfigValue | undefined | null): string {
  if (v === undefined || v === null) return "";
  if (Array.isArray(v)) return `[${v.map(showValue).join(", ")}]`;
  return String(v);
}

/** Text shown when editing an existing value. */
export function inputText(v: ConfigValue | undefined): string {
  if (v === undefined) return "";
  return typeof v === "string" ? v : showValue(v);
}

const isScalar = (x: unknown): x is ConfigScalar =>
  typeof x === "boolean" || typeof x === "number" || typeof x === "string";

/** Typed text is a TOML value (number, boolean, array, quoted string) when it parses as one, else a bare string. */
export function parseInput(text: string): ConfigValue {
  const t = text.trim();
  try {
    const v: unknown = JSON.parse(t);
    if (isScalar(v) || (Array.isArray(v) && v.every(isScalar))) return v as ConfigValue;
  } catch {
    // not a JSON-compatible TOML value: a bare string
  }
  return t;
}

/** `null` when ok, else the message to show next to the input. */
export function checkRange(range: [number, number] | null, v: ConfigValue): string | null {
  if (!range || typeof v !== "number") return null;
  return v < range[0] || v > range[1] ? `Must be between ${range[0]} and ${range[1]}.` : null;
}

export function inputHint(f: Field): string {
  const parts: string[] = [];
  if (f.range) parts.push(`Range: ${f.range[0]} to ${f.range[1]}.`);
  if (f.default !== null) parts.push(`Default: ${showValue(f.default)}.`);
  else if (f.default_text) parts.push(`Default: ${f.default_text}.`);
  parts.push("Empty resets to the default.");
  return parts.join(" ");
}

/** Everything the form derives from the metadata and the draft's values. */
export class Catalog {
  readonly sections: SectionBundle[];
  readonly assists: Assist[];
  readonly fields: Field[];
  private readonly byPath = new Map<string, Field>();

  constructor(meta: MetaBundle) {
    this.sections = meta.sections;
    this.assists = meta.assists;
    this.fields = meta.sections.flatMap((s) => s.fields);
    for (const f of this.fields) this.byPath.set(f.path, f);
  }

  field(path: string): Field | undefined {
    return this.byPath.get(path);
  }

  section(name: string): SectionBundle | undefined {
    return this.sections.find((s) => s.name === name);
  }

  assist(id: string): Assist | undefined {
    return this.assists.find((a) => a.id === id);
  }

  /** Defaults overlaid with the values the draft sets. */
  resolved(set: Values): Values {
    const out: Values = {};
    for (const f of this.fields)
      if (f.kind === "value" && f.default !== null) out[f.path] = f.default;
    return { ...out, ...set };
  }

  /** Whether the field's own `show_if` and those of all ancestor groups hold. */
  isVisible(path: string, resolved: Values): boolean {
    const f = this.field(path);
    if (!f) return false;
    if (f.show_if && !evalCondition(f.show_if, (k) => resolved[k])) return false;
    const i = path.lastIndexOf(".");
    const parent = i < 0 ? null : path.slice(0, i);
    return parent && this.field(parent) ? this.isVisible(parent, resolved) : true;
  }

  private applies(mode: Mode, appliesTo: Command[]): boolean {
    return appliesTo.length === 0 || appliesTo.some((c) => MODE_COMMANDS[mode].includes(c));
  }

  /** Why the field is hidden, or `null` when shown. */
  hiddenReason(f: Field, mode: Mode, resolved: Values): string | null {
    const sec = this.section(f.path.split(".")[0]);
    if ((sec && !this.applies(mode, sec.applies_to)) || !this.applies(mode, f.applies_to))
      return "other mode";
    if (!this.isVisible(f.path, resolved)) return "not applicable to the current choices";
    return null;
  }

  isRequired(f: Field, mode: Mode, resolved: Values): boolean {
    if (!this.isVisible(f.path, resolved)) return false;
    return MODE_COMMANDS[mode].some(
      (c) =>
        f.required_for.includes(c) ||
        (f.required_if !== null && evalCondition(f.required_if, (k) => resolved[k])),
    );
  }

  /** The options of `f` whose own `show_if` holds. */
  visibleOptions(f: Field, resolved: Values): OptionMeta[] {
    return f.options.filter((o) => !o.show_if || evalCondition(o.show_if, (k) => resolved[k]));
  }

  /** The key of the option the value selects, if any. */
  optionKeyOf(f: Field, v: ConfigValue | undefined): string | null {
    return f.options.find((o) => sameOption(f.path, optionValue(o.key), v))?.key ?? null;
  }

  /** The preset whose values all match the resolved values. */
  matchingPreset(assistId: string, resolved: Values): string | null {
    const a = this.assist(assistId);
    const hit = a?.presets.find((p) =>
      Object.entries(p.values).every(([k, v]) => sameOption(k, resolved[k], v)),
    );
    return hit?.label ?? null;
  }
}

/** Case-insensitive match over label, key path, summary and description. */
export function matchesSearch(f: Field, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  return [f.key, f.path, f.summary, f.description].some((s) => s.toLowerCase().includes(q));
}

/** The first segment of an issue or finding path. */
export function sectionOf(path: string | null): string | null {
  return path ? path.split(".")[0] : null;
}
