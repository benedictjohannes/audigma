import { expect, test } from "bun:test";
import { SettingsSchema } from "./generated/zod";

const minimal = { schema_version: 1, target: { format: "pcm", pcm_rate_hz: 96000 } };

test("accepts a minimal configuration", () => {
  expect(SettingsSchema.safeParse(minimal).success).toBe(true);
});

test("accepts the mixed-type keys", () => {
  const cfg = {
    ...minimal,
    resampler: { apodization: 19000 },
    runtime: { dsp_threads: "inline" },
  };
  expect(SettingsSchema.safeParse(cfg).success).toBe(true);
  expect(SettingsSchema.safeParse({ ...cfg, resampler: { apodization: "gentle" } }).success).toBe(true);
  expect(SettingsSchema.safeParse({ ...cfg, runtime: { dsp_threads: 4 } }).success).toBe(true);
});

test("rejects unknown keys and bad enum values", () => {
  expect(SettingsSchema.safeParse({ ...minimal, bogus: 1 }).success).toBe(false);
  expect(SettingsSchema.safeParse({ ...minimal, target: { format: "mp3" } }).success).toBe(false);
  expect(SettingsSchema.safeParse({ ...minimal, runtime: { dsp_threads: 0 } }).success).toBe(false);
  expect(SettingsSchema.safeParse({ ...minimal, resampler: { apodization: 12345 } }).success).toBe(false);
});
