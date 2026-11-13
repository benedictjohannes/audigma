import { describe, expect, test } from "bun:test";
import type { Field, MetaBundle } from "../generated/types";
import {
  Catalog,
  checkRange,
  flatten,
  matchesSearch,
  modeForUseCase,
  optionValue,
  parseInput,
} from "./catalog";

const base: Field = {
  also_integer: null,
  applies_to: [],
  assist: [],
  default: null,
  default_text: null,
  description: "",
  key: "k",
  kind: "value",
  options: [],
  path: "s.k",
  range: null,
  recommended: {},
  required_for: [],
  required_if: null,
  show_if: null,
  summary: "",
};
const f = (o: Partial<Field>): Field => ({ ...base, ...o });
const opt = (key: string) => ({
  key,
  label: key,
  summary: "",
  description: "",
  caution: null,
  show_if: null,
});

const meta: MetaBundle = {
  assists: [
    {
      id: "preset",
      kind: "preset",
      label: "Starting point",
      fills: [],
      presets: [
        {
          key: "neutral",
          label: "Neutral",
          summary: "",
          values: { "resampler.apodization": "off", "resampler.order": 5 },
        },
      ],
    },
  ],
  sections: [
    {
      name: "s",
      title: "S",
      summary: "",
      description: "",
      advanced: false,
      applies_to: [],
      fields: [
        f({ path: "s.mode", key: "mode", default: "a", options: [opt("a"), opt("b")] }),
        f({ path: "s.grp", key: "grp", kind: "group", show_if: { key: "s.mode", eq: "b" } }),
        f({ path: "s.grp.x", key: "x", required_if: { key: "s.mode", eq: "b" } }),
        f({
          path: "s.offline_only",
          key: "offline_only",
          applies_to: ["convert"],
          required_for: ["convert"],
        }),
      ],
    },
    {
      name: "resampler",
      title: "R",
      summary: "",
      description: "",
      advanced: false,
      applies_to: [],
      fields: [
        f({
          path: "resampler.apodization",
          key: "apodization",
          default: "off",
          options: [opt("off"), opt("gentle"), opt("18000")],
        }),
        f({ path: "resampler.order", key: "order", default: 5 }),
      ],
    },
  ],
};
const cat = new Catalog(meta);

describe("values", () => {
  test("flatten walks nested tables", () => {
    expect(
      flatten({ schema_version: 1, output: { backend: "alsa" }, input: { dsd: { gain_db: 1.5 } } }),
    ).toEqual({
      schema_version: 1,
      "output.backend": "alsa",
      "input.dsd.gain_db": 1.5,
    });
  });
  test("parseInput reads TOML-like scalars and arrays, else a bare string", () => {
    expect(parseInput("5")).toBe(5);
    expect(parseInput("-1.5")).toBe(-1.5);
    expect(parseInput("true")).toBe(true);
    expect(parseInput('"hi"')).toBe("hi");
    expect(parseInput("[1, 2]")).toEqual([1, 2]);
    expect(parseInput("hw:0,0")).toBe("hw:0,0");
    expect(parseInput("null")).toBe("null");
    expect(parseInput("{a=1}")).toBe("{a=1}");
  });
  test("option keys are typed like the metadata", () => {
    expect(optionValue("7")).toBe(7);
    expect(optionValue("true")).toBe(true);
    expect(optionValue("gentle")).toBe("gentle");
  });
  test("range check", () => {
    expect(checkRange([1, 10], 11)).toContain("between 1 and 10");
    expect(checkRange([1, 10], 5)).toBeNull();
    expect(checkRange([1, 10], "x")).toBeNull();
    expect(checkRange(null, 99)).toBeNull();
  });
  test("mode follows the use case", () => {
    expect(modeForUseCase(null)).toBe("all");
    expect(modeForUseCase("offline-conversion")).toBe("offline");
    expect(modeForUseCase("headless-appliance")).toBe("live");
  });
});

describe("catalog", () => {
  const field = (p: string) => {
    const f = cat.field(p);
    if (!f) throw new Error(`no field ${p}`);
    return f;
  };
  test("a group's show_if hides its children", () => {
    expect(cat.hiddenReason(field("s.grp.x"), "all", cat.resolved({}))).toBe(
      "not applicable to the current choices",
    );
    expect(cat.hiddenReason(field("s.grp.x"), "all", cat.resolved({ "s.mode": "b" }))).toBeNull();
  });
  test("applies_to hides keys of the other mode", () => {
    expect(cat.hiddenReason(field("s.offline_only"), "live", {})).toBe("other mode");
    expect(cat.hiddenReason(field("s.offline_only"), "offline", {})).toBeNull();
  });
  test("required: required_for per mode, required_if on resolved values, never when hidden", () => {
    expect(cat.isRequired(field("s.offline_only"), "offline", {})).toBe(true);
    expect(cat.isRequired(field("s.offline_only"), "live", {})).toBe(false);
    expect(cat.isRequired(field("s.grp.x"), "all", cat.resolved({ "s.mode": "b" }))).toBe(true);
    expect(cat.isRequired(field("s.grp.x"), "all", cat.resolved({}))).toBe(false);
  });
  test("resolved overlays the draft on the defaults", () => {
    expect(cat.resolved({ "resampler.order": 7 })["resampler.order"]).toBe(7);
    expect(cat.resolved({})["resampler.apodization"]).toBe("off");
  });
  test("gentle is the stored 19 kHz edge", () => {
    const a = field("resampler.apodization");
    expect(cat.optionKeyOf(a, "gentle")).toBe("gentle");
    expect(cat.optionKeyOf(a, 19000)).toBe("gentle");
    expect(cat.optionKeyOf(a, 18000)).toBe("18000");
    expect(cat.optionKeyOf(a, "nope")).toBeNull();
  });
  test("preset matching", () => {
    expect(cat.matchingPreset("preset", cat.resolved({ "resampler.order": 5 }))).toBe("Neutral");
    expect(cat.matchingPreset("preset", cat.resolved({ "resampler.order": 7 }))).toBeNull();
  });
  test("search covers key, path, summary and description", () => {
    const x = f({
      key: "buffer_ms",
      path: "output.buffer_ms",
      summary: "Ring size",
      description: "in milliseconds",
    });
    expect(matchesSearch(x, "BUFFER")).toBe(true);
    expect(matchesSearch(x, "output.")).toBe(true);
    expect(matchesSearch(x, "ring")).toBe(true);
    expect(matchesSearch(x, "millisec")).toBe(true);
    expect(matchesSearch(x, "volume")).toBe(false);
    expect(matchesSearch(x, "  ")).toBe(true);
  });
});
