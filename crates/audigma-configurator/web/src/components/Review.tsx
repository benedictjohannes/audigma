import { Loader2, Play, Square } from "lucide-react";
import { useEffect, useState } from "react";
import type { CapabilityFinding, DraftView, Issue, ValidateResponse } from "../generated/types";
import type { BenchState } from "../lib/bench";
import { showValue, type Values } from "../lib/catalog";
import { api } from "../lib/client";
import { keyed } from "../lib/keys";
import { Btn } from "./Navbar";
import { Alert, Card, Tag } from "./ui";

const STAGE: Record<string, string> = {
  end_to_end: "end to end",
  sequential: "sequential",
  workers: "workers",
};

function Section({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <Card className="p-4">
      <h2 className="mb-2 text-base font-semibold">{title}</h2>
      <div className="space-y-2">{children}</div>
    </Card>
  );
}

function IssueList({ issues, findings }: { issues: Issue[]; findings: CapabilityFinding[] }) {
  return (
    <>
      {keyed(issues, (i) => `${i.path}:${i.message}`).map(({ key, item: i }) => (
        <Alert key={`i:${key}`} kind={i.error ? "error" : "warning"}>
          {i.path && <code className="mr-1 font-mono">{i.path}:</code>}
          {i.message}
        </Alert>
      ))}
      {keyed(findings, (x) => `${x.path}:${x.message}`).map(({ key, item: x }) => (
        <Alert
          key={`f:${key}`}
          kind={x.severity === "error" ? "error" : x.severity === "warning" ? "warning" : "info"}
        >
          <code className="mr-1 font-mono">{x.path}:</code>
          {x.message}
        </Alert>
      ))}
    </>
  );
}

export function Review(p: {
  draft: DraftView;
  set: Values;
  bench: BenchState;
  onStart: () => void;
  onCancel: () => void;
  refresh: () => void;
  onForm: () => void;
  onSave: () => void;
  banner?: boolean;
  locked: boolean;
}) {
  const { draft } = p;
  // Always validate when the page opens (the server's answer is authoritative).
  const [checked, setChecked] = useState<ValidateResponse | null>(null);
  // Re-validate whenever the draft's content changes (e.g. a file loaded over it), not just its counts.
  const sig = JSON.stringify(draft.values);
  // biome-ignore lint/correctness/useExhaustiveDependencies: the signature only triggers a re-validation
  useEffect(() => {
    api.validate().then(setChecked, () => setChecked(null));
  }, [sig]);
  const issues = checked?.issues ?? draft.issues;

  const [probing, setProbing] = useState(false);
  const [probeError, setProbeError] = useState<string | null>(null);
  const probe = async () => {
    setProbing(true);
    try {
      const r = await api.probeAlsa();
      setProbeError(r.error);
      p.refresh();
    } finally {
      setProbing(false);
    }
  };

  const hasErrors = issues.some((i) => i.error);
  const v = draft.validation;
  const running = p.bench.state === "running";

  return (
    <div className="space-y-3">
      {p.banner && (
        <Alert kind="success" label="These are the values guided setup chose.">
          Open the form to edit any of them.{" "}
          <button type="button" className="text-primary underline" onClick={p.onForm}>
            Open the form
          </button>
        </Alert>
      )}

      <Section title="Problems">
        {issues.length === 0 ? (
          <Alert kind="success">No problems found.</Alert>
        ) : (
          <IssueList issues={issues} findings={[]} />
        )}
      </Section>

      <Section title="Device check">
        <p className="text-muted">
          Compares the configuration with the ALSA devices on the computer where audigma is running.
        </p>
        <Btn onClick={() => void probe()} disabled={probing}>
          {probing ? <Loader2 size={14} className="animate-spin" aria-hidden /> : null} Detect
          devices and check
        </Btn>
        {(probeError ??
          (draft.device_check.state === "failed" ? draft.device_check.message : null)) && (
          <Alert kind="warning" label="ALSA probe failed:">
            {probeError ??
              (draft.device_check.state === "failed" ? draft.device_check.message : "")}
          </Alert>
        )}
        {draft.device_check.state === "not_run" && (
          <p className="text-muted">
            No device check has run yet. Press the button to compare with the devices found now.
          </p>
        )}
        {draft.device_check.state === "blocked" && (
          <p className="text-muted">
            The configuration has errors, so it cannot be checked against a device yet.
          </p>
        )}
        {draft.device_check.state === "checked" && draft.findings.length === 0 && (
          <Alert kind="success">Checked against the devices found; no findings.</Alert>
        )}
        <IssueList issues={[]} findings={draft.findings} />
      </Section>

      <Section title="Validate on this machine (an estimate)">
        <p className="text-muted">
          Runs the resampler on this machine with exactly this configuration. Passing is necessary,
          not sufficient, for live playback.
        </p>
        <div className="flex items-center gap-2">
          {running ? (
            <Btn danger onClick={p.onCancel}>
              <Square size={14} aria-hidden /> Cancel
            </Btn>
          ) : (
            <Btn
              primary
              onClick={p.onStart}
              disabled={hasErrors || p.locked}
              title={hasErrors ? "Fix the errors first" : undefined}
            >
              <Play size={14} aria-hidden /> Validate
            </Btn>
          )}
          {p.bench.state === "running" && (
            <>
              <progress
                className="w-48"
                value={p.bench.fraction}
                max={1}
                aria-label="Benchmark progress"
              />
              <span className="text-muted">
                {p.bench.stage ? STAGE[p.bench.stage] : "starting"}
              </span>
            </>
          )}
        </div>
        {p.bench.state === "failed" && (
          <Alert kind="error" label="Failed:">
            {p.bench.message}
          </Alert>
        )}
        {p.bench.state === "cancelled" && <p className="text-muted">Cancelled.</p>}
        {v && !running && (
          <div className="space-y-2">
            <div className="flex flex-wrap items-center gap-2">
              {v.result.gates.p999 && v.result.gates.throughput ? (
                <Tag tone="success">pass</Tag>
              ) : (
                <Tag tone="danger">fail</Tag>
              )}
              {v.stale && <Tag tone="warning">stale: the configuration changed since this run</Tag>}
            </div>
            <p>
              {v.result.worker_throughput_x.toFixed(1)}x real time (needs 2.0x), p99.9{" "}
              {v.result.p999_budget_percent.toFixed(0)}% of the block budget (max 70%).
            </p>
            <p className="text-xs text-muted">
              {v.result.target_format}, input {v.result.input_rate_hz} Hz, filter {v.result.filter},
              end to end {v.result.end_to_end_throughput_x.toFixed(1)}x, sequential{" "}
              {v.result.sequential_throughput_x.toFixed(1)}x.
            </p>
          </div>
        )}
      </Section>

      <Section title={`Changed keys (${draft.touched.length})`}>
        {draft.touched.length === 0 ? (
          <p className="text-muted">Nothing changed yet.</p>
        ) : (
          <ul className="font-mono">
            {draft.touched.map((k) => (
              <li key={k}>
                {k} ={" "}
                {k in p.set ? (
                  showValue(p.set[k])
                ) : (
                  <span className="text-muted">(reset to default)</span>
                )}
              </li>
            ))}
          </ul>
        )}
      </Section>

      <div className="flex flex-wrap justify-end gap-2">
        <Btn onClick={p.onForm}>Edit</Btn>
        <Btn primary onClick={p.onSave} disabled={issues.some((i) => i.error)}>
          Save
        </Btn>
      </div>
    </div>
  );
}
