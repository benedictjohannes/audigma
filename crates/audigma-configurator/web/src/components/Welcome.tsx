import { FileUp, ListChecks, Pencil, Upload } from "lucide-react";
import { type ReactNode, useRef, useState } from "react";
import type { UseCase } from "../generated/types";
import { USE_CASES } from "../lib/catalog";
import { Btn } from "./Navbar";
import { Alert, Card, Modal } from "./ui";

/** Upload or paste a TOML file. `onLoad` resolves with an error message, or `null` on success. */
export function LoadPanel({
  onLoad,
  compact,
}: {
  onLoad: (text: string, name?: string) => Promise<string | null>;
  compact?: boolean;
}) {
  const [text, setText] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [over, setOver] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  const rows = compact ? 4 : 4;

  const run = async (t: string, name?: string) => setError(await onLoad(t, name));
  const file = async (f: File | undefined) => {
    if (f) await run(await f.text(), f.name);
  };

  return (
    <div className="space-y-3">
      {/* biome-ignore lint/a11y/noStaticElementInteractions: drop target; the file button is the keyboard path */}
      <div
        onDragOver={(e) => {
          e.preventDefault();
          setOver(true);
        }}
        onDragLeave={() => setOver(false)}
        onDrop={(e) => {
          e.preventDefault();
          setOver(false);
          void file(e.dataTransfer.files[0]);
        }}
        className={`flex h-24 flex-col items-center justify-center gap-1 rounded-ant border border-dashed text-center ${
          over ? "border-primary bg-primary-bg" : "border-line"
        }`}
      >
        <Upload size={18} aria-hidden className="text-muted" />
        <span className="text-muted">Drop a config file here, or</span>
        <input
          ref={input}
          type="file"
          accept=".toml,text/plain,application/toml"
          className="sr-only"
          aria-label="Upload a config file"
          onChange={(e) => void file(e.target.files?.[0])}
        />
        <Btn onClick={() => input.current?.click()}>
          <FileUp size={14} aria-hidden /> Choose file…
        </Btn>
      </div>
      <div className="space-y-2">
        <textarea
          aria-label="Paste TOML"
          rows={rows}
          placeholder="…or paste TOML here"
          className="w-full rounded-ant border border-line bg-card p-2 font-mono"
          value={text}
          onChange={(e) => setText(e.target.value)}
        />
        <Btn primary disabled={text.trim() === ""} onClick={() => void run(text)}>
          Load
        </Btn>
      </div>
      {error && <Alert kind="error">{<span className="whitespace-pre-wrap">{error}</span>}</Alert>}
    </div>
  );
}

function Option({
  icon,
  title,
  children,
}: {
  icon: ReactNode;
  title: string;
  children?: ReactNode;
}) {
  return (
    <Card className="p-4">
      <h2 className="mb-2 flex items-center gap-2 text-base font-semibold">
        {icon} {title}
      </h2>
      {children}
    </Card>
  );
}

/** The screen for a path that does not exist (W13): four cards. */
export function Welcome(p: {
  path: string;
  onLoad: (text: string, name?: string) => Promise<string | null>;
  onGuided: () => void;
  onEdit: () => void;
  disabled: boolean;
}) {
  return (
    <main className="flex min-h-[calc(100vh-3.5rem)] items-center justify-center p-4">
      <div className="w-full max-w-120 space-y-3">
        <p className="text-muted">
          <code className="font-mono">{p.path}</code> does not exist yet. Start from one of these.
        </p>
        <Option icon={<Upload size={16} aria-hidden />} title="Upload a file">
          <LoadPanel onLoad={p.onLoad} compact />
        </Option>
        <Option icon={<ListChecks size={16} aria-hidden />} title="Guided setup">
          <div className="flex items-center justify-between gap-2">
            <span className="text-muted">A few questions, then a review.</span>
            <Btn primary onClick={p.onGuided} disabled={p.disabled}>
              Start
            </Btn>
          </div>
        </Option>
        <Option icon={<Pencil size={16} aria-hidden />} title="Edit configuration">
          <div className="flex items-center justify-between gap-2">
            <span className="text-muted">Open the full form.</span>
            <Btn onClick={p.onEdit} disabled={p.disabled}>
              Open
            </Btn>
          </div>
        </Option>
      </div>
    </main>
  );
}

/** The skippable use-case question that precedes direct editing of a new file. */
export function UseCaseDialog({
  onPick,
  onSkip,
}: {
  onPick: (u: UseCase) => void;
  onSkip: () => void;
}) {
  return (
    <Modal title="What will you use this for?" onClose={onSkip}>
      <p className="mb-3 text-muted">
        This only highlights recommendations and picks which keys show first. It is never written to
        the file.
      </p>
      <ul className="space-y-2">
        {USE_CASES.map((u) => (
          <li key={u.id}>
            <button
              type="button"
              onClick={() => onPick(u.id)}
              className="block w-full rounded-ant border border-line px-3 py-2 text-left hover:border-primary"
            >
              <span className="font-semibold">{u.label}</span>
              <span className="block text-muted">{u.blurb.replaceAll("`", "")}</span>
            </button>
          </li>
        ))}
      </ul>
      <div className="mt-3 text-right">
        <Btn onClick={onSkip}>Skip</Btn>
      </div>
    </Modal>
  );
}

/** *Load file…* in the form navbar: the same panel in a dialog. */
export function LoadDialog({
  onLoad,
  onClose,
  dirty,
}: {
  onLoad: (text: string, name?: string) => Promise<string | null>;
  onClose: () => void;
  dirty: boolean;
}) {
  return (
    <Modal title="Load a file into the draft" onClose={onClose}>
      {dirty && (
        <div className="mb-3">
          <Alert kind="warning">
            Loading replaces the draft. Your changes that were neither saved nor downloaded will be
            lost.
          </Alert>
        </div>
      )}
      <LoadPanel onLoad={onLoad} />
      <div className="mt-3 text-right">
        <Btn onClick={onClose}>Cancel</Btn>
      </div>
    </Modal>
  );
}
