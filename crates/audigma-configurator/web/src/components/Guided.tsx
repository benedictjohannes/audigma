import { Loader2 } from "lucide-react";
import type { GuidedState, TierView } from "../generated/types";
import { Markdown } from "./Markdown";
import { Btn } from "./Navbar";
import { Alert, Card, Tag, type Tone } from "./ui";

function tierTag(t: TierView): { tone: Tone; text: string } {
  switch (t.state) {
    case "passed":
      return {
        tone: "success",
        text: `passed, ${t.throughput_x.toFixed(1)}x, p99.9 ${t.p999_percent.toFixed(0)}%`,
      };
    case "failed":
      return {
        tone: "danger",
        text: `failed, ${t.throughput_x.toFixed(1)}x, p99.9 ${t.p999_percent.toFixed(0)}%`,
      };
    case "error":
      return { tone: "danger", text: t.message };
    case "running":
      return { tone: "primary", text: "running" };
    case "cancelled":
      return { tone: "default", text: "cancelled" };
    default:
      return { tone: "default", text: "not tested" };
  }
}

/** The discovery sweep so far (results are labelled estimates). */
function Discovery({ state, onCancel }: { state: GuidedState["discovery"]; onCancel: () => void }) {
  if (state.tiers.length === 0 && !state.active) return null;
  return (
    <Card className="space-y-2 p-3">
      <div className="flex items-center gap-2">
        <strong>Compute capability (estimate)</strong>
        {state.stale && <Tag tone="warning">stale: quality settings changed since the sweep</Tag>}
        {state.active && (
          <>
            <progress
              className="ml-auto w-40"
              value={state.progress}
              max={1}
              aria-label="Discovery progress"
            />
            <Btn danger onClick={onCancel}>
              Cancel
            </Btn>
          </>
        )}
      </div>
      <ul className="space-y-1">
        {state.tiers.map((t) => {
          const g = tierTag(t);
          return (
            <li key={t.key} className="flex items-center gap-2">
              <span className="w-40">{t.label}</span>
              <Tag tone={g.tone}>{g.text}</Tag>
            </li>
          );
        })}
      </ul>
    </Card>
  );
}

/** One guided step: prompt, a radio list of the choices the server gives, discovery results. */
export function GuidedStep({
  state,
  selected,
  onChoose,
  onCancelDiscovery,
  busy,
}: {
  state: GuidedState;
  selected: string | null;
  /** Picking a choice answers the step. */
  onChoose: (key: string) => void;
  onCancelDiscovery: () => void;
  busy: boolean;
}) {
  const step = state.step;
  if (!step) return <Alert kind="info">Guided setup is not running.</Alert>;
  const swept = step.kind === "discover" && state.discovery.tiers.length > 0;
  const label = (key: string, text: string) =>
    swept ? (key === "run" ? "Run it again" : key === "skip" ? "Continue" : text) : text;
  const showDiscovery = step.kind === "discover" || step.kind === "tiers";
  return (
    <div className="space-y-3">
      <div>
        <h2 className="text-xl font-semibold">{step.title}</h2>
      </div>
      <div className="text-base">
        <Markdown text={step.prompt} />
      </div>
      {step.waiting_for_probe && (
        <p className="flex items-center gap-2 text-muted">
          <Loader2 size={14} className="animate-spin" aria-hidden /> Looking at the devices…
        </p>
      )}
      {step.problem && <Alert kind="warning">{step.problem}</Alert>}
      {showDiscovery && <Discovery state={state.discovery} onCancel={onCancelDiscovery} />}
      <fieldset className="space-y-2">
        <legend className="sr-only">{step.title}</legend>
        {step.choices.map((c) => {
          const on = selected === c.key;
          const off = !!c.disabled || busy;
          return (
            // biome-ignore lint/a11y/useKeyWithClickEvents: outer card click; button handles keyboard
            // biome-ignore lint/a11y/noStaticElementInteractions: outer card click; button handles keyboard
            <div
              key={c.key}
              onClick={(e) => {
                if (off) return;
                const target = e.target as HTMLElement;
                if (target.closest("a, button")) return;
                if (window.getSelection()?.toString()) return;
                onChoose(c.key);
              }}
              className={`flex gap-3 rounded-ant border bg-card px-3 py-2 has-focus-visible:ring-2 has-focus-visible:ring-primary ${
                off ? "cursor-not-allowed opacity-60" : "cursor-pointer"
              } ${on ? "border-primary bg-primary-bg" : "border-line-soft hover:border-primary"}`}
            >
              <span
                aria-hidden
                className={`mt-1.5 h-3.5 w-3.5 shrink-0 rounded-full border ${on ? "border-primary bg-primary" : "border-line"}`}
              />
              <div className="min-w-0 space-y-1">
                <button
                  type="button"
                  aria-pressed={on}
                  disabled={off}
                  onClick={() => onChoose(c.key)}
                  className="flex flex-wrap items-center gap-2 text-left outline-none disabled:cursor-not-allowed"
                >
                  <span className="font-semibold">{label(c.key, c.label)}</span>
                  {c.recommendation && <Tag tone="success">{c.recommendation}</Tag>}
                  {c.disabled && <Tag tone="danger">unavailable</Tag>}
                </button>
                {c.summary && (
                  <div className="text-muted">
                    <Markdown text={c.summary} />
                  </div>
                )}
                {c.disabled && <span className="block text-danger">{c.disabled}</span>}
                {c.note && <Markdown text={c.note} />}
                {c.description && <Markdown text={c.description} />}
                {c.caution && (
                  <span className="block">
                    <Alert kind="warning" label="Caution:">
                      {c.caution}
                    </Alert>
                  </span>
                )}
              </div>
            </div>
          );
        })}
      </fieldset>
    </div>
  );
}
