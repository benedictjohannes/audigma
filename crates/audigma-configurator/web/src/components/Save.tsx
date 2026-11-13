import { Download, Loader2, Save as SaveIcon } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import type { CommentLevel, PreviewResponse, SaveResponse } from "../generated/types";
import { ApiFailure, api } from "../lib/client";
import { diffLines, diffStats } from "../lib/diff";
import { keyed } from "../lib/keys";
import { Btn } from "./Navbar";
import { Alert, Card, Tag } from "./ui";

export interface SaveOptions {
  path: string | null;
  only_changed: boolean;
  comment_level: CommentLevel;
}

const LEVELS: { id: CommentLevel; label: string }[] = [
  { id: "none", label: "None" },
  { id: "brief", label: "Brief" },
  { id: "full", label: "Full" },
];

/** The commands to try next. After a download the path is for the user to fill in. */
export function Commands({ commands }: { commands: string[] }) {
  return (
    <pre className="overflow-auto rounded-ant bg-code p-3 font-mono">{commands.join("\n")}</pre>
  );
}

export function downloadCommands(filename: string): string[] {
  const p = `<path to ${filename}>`;
  return [`audigma play -i <file> -c ${p}`, `audigma run -c ${p}`];
}

function Diff({ before, after }: { before: string; after: string }) {
  const d = useMemo(() => diffLines(before, after), [before, after]);
  const s = diffStats(d);
  return (
    <div className="space-y-1">
      <p className="text-muted">
        <span className="text-success">+{s.added}</span>{" "}
        <span className="text-danger">−{s.removed}</span> lines against the existing file
      </p>
      <pre className="max-h-112 overflow-auto rounded-ant border border-line-soft bg-code py-1 font-mono">
        {keyed(d, (l) => `${l.kind}:${l.text}`).map(({ key, item: l }) => (
          <div
            key={key}
            className={`px-3 whitespace-pre-wrap ${l.kind === "add" ? "bg-success/15" : l.kind === "del" ? "bg-danger/15" : "text-muted"}`}
          >
            {l.kind === "add" ? "+ " : l.kind === "del" ? "- " : "  "}
            {l.text}
          </div>
        ))}
      </pre>
    </div>
  );
}

export function SavePage(p: {
  opts: SaveOptions;
  onOpts: (o: SaveOptions) => void;
  hasErrors: boolean;
  stopped: boolean;
  saved: SaveResponse | null;
  onSaved: (r: SaveResponse) => void;
  onDownload: (pv: PreviewResponse) => void;
  downloaded: string | null;
  locked: boolean;
}) {
  const { opts, onOpts, saved, stopped } = p;
  const [pv, setPv] = useState<PreviewResponse | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // Preview on open and whenever an option changes (nothing is written).
  useEffect(() => {
    if (saved || stopped) return;
    let live = true;
    const t = setTimeout(() => {
      api
        .preview({
          path: opts.path,
          only_changed: opts.only_changed,
          comment_level: opts.comment_level,
        })
        .then(
          (r) => {
            if (!live) return;
            setPv(r);
            setErr(null);
            // Prefill the path with the resolved default.
            if (opts.path === null) onOpts({ ...opts, path: r.path });
          },
          (e) => {
            if (!live) return;
            setPv(null);
            setErr(e instanceof Error ? e.message : String(e));
          },
        );
    }, 250);
    return () => {
      live = false;
      clearTimeout(t);
    };
  }, [opts, onOpts, saved, stopped]);

  const save = async () => {
    setBusy(true);
    try {
      p.onSaved(
        await api.save({
          path: opts.path,
          only_changed: opts.only_changed,
          comment_level: opts.comment_level,
        }),
      );
    } catch (e) {
      setErr(e instanceof ApiFailure || e instanceof Error ? e.message : String(e));
    } finally {
      setBusy(false);
    }
  };

  if (p.saved) {
    const s = p.saved;
    return (
      <div className="space-y-3">
        <Alert
          kind="success"
          label={s.unchanged ? "Nothing to write." : s.created ? "Created the file." : "Saved."}
        >
          <code className="font-mono">{s.path}</code>
          {s.backup && (
            <>
              {" "}
              The previous file was kept as <code className="font-mono">{s.backup}</code>.
            </>
          )}
        </Alert>
        <Card className="space-y-2 p-4">
          <h2 className="text-base font-semibold">Try it</h2>
          <Commands commands={s.next} />
          <p className="text-muted">The configurator has closed; you can close this tab.</p>
        </Card>
      </div>
    );
  }

  const field = "h-8 rounded-ant border border-line bg-card px-2 text-fg";
  return (
    <div className="space-y-3">
      <Card className="space-y-4 p-4">
        <label className="block space-y-1">
          <span className="font-semibold">Save to</span>
          <span className="block text-muted">
            A file path on the computer where you started audigma.
          </span>
          <input
            className={`${field} w-full font-mono`}
            value={opts.path ?? ""}
            onChange={(e) => p.onOpts({ ...opts, path: e.target.value || null })}
          />
        </label>
        <label className="flex items-center gap-2">
          <input
            type="checkbox"
            checked={opts.only_changed}
            onChange={(e) => p.onOpts({ ...opts, only_changed: e.target.checked })}
          />
          Write only changed keys (leave out values equal to their default)
        </label>
        <fieldset className="space-y-1">
          <legend className="font-semibold">Comments</legend>
          <div className="flex gap-2">
            {LEVELS.map((l) => (
              <Btn
                key={l.id}
                primary={opts.comment_level === l.id}
                onClick={() => p.onOpts({ ...opts, comment_level: l.id })}
              >
                {l.label}
              </Btn>
            ))}
          </div>
          <p className="text-muted">
            Lines starting with <code className="font-mono">##</code> are generated and rewritten on
            every save; lines starting with <code className="font-mono">#</code> are yours and are
            kept.
          </p>
        </fieldset>
      </Card>

      {p.hasErrors && (
        <Alert kind="error">Fix the errors before saving; see the review page.</Alert>
      )}
      {err && <Alert kind="error">{err}</Alert>}

      <Card className="space-y-2 p-4">
        <h2 className="flex items-center gap-2 text-base font-semibold">
          Preview{" "}
          {pv && (pv.exists ? <Tag>existing file</Tag> : <Tag tone="primary">new file</Tag>)}
        </h2>
        {!pv && !err && (
          <p className="flex items-center gap-2 text-muted">
            <Loader2 size={14} className="animate-spin" aria-hidden /> Rendering…
          </p>
        )}
        {pv?.exists && pv.existing !== null && pv.unchanged && (
          <Alert kind="info">Nothing to write: the file already matches.</Alert>
        )}
        {pv?.exists && pv.existing !== null && !pv.unchanged && (
          <Diff before={pv.existing} after={pv.text} />
        )}
        {pv && !pv.exists && (
          <pre className="max-h-112 overflow-auto rounded-ant border border-line-soft bg-code p-3 font-mono whitespace-pre-wrap">
            {pv.text}
          </pre>
        )}
      </Card>

      {p.downloaded && (
        <Card className="space-y-2 p-4">
          <h2 className="text-base font-semibold">Downloaded {p.downloaded}</h2>
          <p className="text-muted">Put it where you want it, then try (fill in the path):</p>
          <Commands commands={downloadCommands(p.downloaded)} />
        </Card>
      )}

      <div className="flex flex-wrap gap-2">
        <Btn
          primary
          onClick={() => void save()}
          disabled={busy || p.hasErrors || p.stopped || p.locked || !pv}
        >
          <SaveIcon size={14} aria-hidden /> Save file
        </Btn>
        <Btn onClick={() => pv && p.onDownload(pv)} disabled={p.hasErrors || p.stopped || !pv}>
          <Download size={14} aria-hidden /> Download
        </Btn>
      </div>
      <p className="text-muted">
        <strong>Save file</strong> writes the file to the path above and then closes the
        configurator. <strong>Download</strong> gives your browser a copy instead, writes nothing
        and keeps the configurator open.
      </p>
    </div>
  );
}
