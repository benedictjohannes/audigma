import { expect, test } from "bun:test";
import { ApiRequestsSchema, ApiResponsesSchema } from "./generated/zod";
import type { AlsaProbeResponse, DraftView, JobEvent } from "./generated/types";

const res = ApiResponsesSchema.shape;

// Shapes as the server emits them (keys with no value are left out, as in the Rust output).
const k17: AlsaProbeResponse = {
  error: null,
  report: {
    cards: [
      {
        index: 3,
        id: "K17",
        name: "USB-Audio",
        longname: "FiiO K17",
        usb_id: "2972:0107",
        devices: [
          {
            card_index: 3,
            device_index: 0,
            id: "USB Audio",
            name: "USB Audio",
            device_path: "hw:CARD=K17,DEV=0",
            is_busy: false,
            capabilities: {
              formats: ["S16_LE", "S32_LE", "DSD_U32_BE"],
              rates_hz: [44100, 48000],
              min_channels: 2,
              max_channels: 2,
              native_dsd: { supported: true, formats: ["DSD_U32_BE"], max_frame_rate_hz: 768000 },
              dop: { supported: true, supported_containers: ["s32le"] },
            },
          },
        ],
      },
    ],
  },
};

test("the ALSA probe response is typed and parses", () => {
  expect(res.probe_alsa.safeParse(k17).success).toBe(true);
  const broken = structuredClone(k17);
  (broken.report!.cards[0].devices[0] as { is_busy: unknown }).is_busy = "no";
  expect(res.probe_alsa.safeParse(broken).success).toBe(false);
});

test("the PipeWire probe response and a failed probe parse", () => {
  const sink = {
    id: 52,
    node_name: "alsa_output.usb-FiiO_K17",
    description: "K17",
    device_api: "alsa",
    alsa_card: 3,
    alsa_card_name: "K17",
    alsa_path: null,
  };
  expect(res.probe_pipewire.safeParse({ report: { sinks: [sink] }, error: null }).success).toBe(true);
  expect(res.probe_pipewire.safeParse({ report: null, error: "no PipeWire daemon" }).success).toBe(true);
  expect(res.probe_pipewire.safeParse({ report: { sinks: [{ id: "x" }] }, error: null }).success).toBe(false);
});

test("benchmark events are a tagged union", () => {
  const progress: JobEvent = { type: "progress", stage: "workers", fraction: 0.5, tier: "dsd128" };
  expect(res.job_event.safeParse(progress).success).toBe(true);
  expect(res.job_event.safeParse({ type: "cancelled" }).success).toBe(true);
  expect(res.job_event.safeParse({ type: "failed", message: "x" }).success).toBe(true);
  expect(res.job_event.safeParse({ type: "progress", stage: "nope", fraction: 0, tier: null }).success).toBe(false);
  const sweep: JobEvent = {
    type: "discovery",
    discovery: {
      active: true,
      progress: 0.2,
      stale: false,
      tiers: [
        { key: "dsd64", label: "DSD64", state: "passed", throughput_x: 6.1, p999_percent: 12 },
        { key: "dsd128", label: "DSD128", state: "running" },
        { key: "dsd256", label: "DSD256", state: "error", message: "cannot be applied" },
      ],
    },
  };
  expect(res.job_event.safeParse(sweep).success).toBe(true);
});

test("a draft carries scalar and list values", () => {
  const draft: DraftView = {
    path: "/tmp/config.toml",
    exists: false,
    is_new: true,
    use_case: null,
    guided_active: false,
    values: {
      schema_version: 1,
      input: { allowed_rates: [44100, 48000], allow_dsd: true },
      target: { format: "pcm", pcm_rate_hz: 88200 },
    },
    touched: ["target.format"],
    issues: [{ path: "target", message: "the configuration must specify [target]", error: true }],
    findings: [],
    validation: null,
  };
  expect(res.draft.safeParse(draft).success).toBe(true);
  // The tree is recursive, which the zod generator renders as `any` below the top level (see
  // gen.ts); the TypeScript type `DraftView["values"]` is the strict one.
  expect(res.draft.safeParse({ ...draft, values: "nope" }).success).toBe(false);
});

test("request bodies are typed", () => {
  const req = ApiRequestsSchema.shape;
  expect(req.draft_edit.safeParse({ set: { "target.pcm_rate_hz": 88200 }, use_case: null }).success).toBe(true);
  expect(req.draft_edit.safeParse({ set: { "target.format": { nested: 1 } } }).success).toBe(false);
  expect(req.save.safeParse({ comment_level: "full", only_changed: false }).success).toBe(true);
  expect(req.save.safeParse({ comment_level: "loud" }).success).toBe(false);
  expect(req.guided_answer.safeParse({}).success).toBe(false);
});
