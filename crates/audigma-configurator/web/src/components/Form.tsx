import { ChevronDown, ChevronRight, RotateCcw, Search as SearchIcon, Wand2 } from "lucide-react";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import type {
  AlsaProbeReport,
  CapabilityFinding,
  ConfigValue,
  Field,
  Issue,
  PipeWireProbeReport,
  UseCase,
} from "../generated/types";
import {
  type Catalog,
  checkRange,
  inputHint,
  inputText,
  isBoolField,
  labelOfUseCase,
  type Mode,
  matchesSearch,
  optionValue,
  parseInput,
  sectionOf,
  showValue,
  type Values,
} from "../lib/catalog";
import { api } from "../lib/client";
import { keyed } from "../lib/keys";
import { Markdown } from "./Markdown";
import { Btn } from "./Navbar";
import { Alert, Switch, Tag } from "./ui";

export interface FormProps {
  catalog: Catalog;
  mode: Mode;
  showHidden: boolean;
  search: string;
  useCase: UseCase | null;
  set: Values;
  touched: Set<string>;
  issues: Issue[];
  findings: CapabilityFinding[];
  /** Why editing is off (a benchmark or guided setup runs), or `null`. */
  locked: string | null;
  /** Server-side refusals of the last edit, by key. */
  fieldErrors: Record<string, string>;
  /** Sends one change; resolves `true` when the server accepted it. */
  edit: (set: Values, unset: string[]) => Promise<boolean>;
  /** Re-reads the draft (after a probe, whose findings the server folds in). */
  refresh: () => void;
  onReview: () => void;
  onSave: () => void;
  /** A section to open (guided setup ends on `[offline]`). */
  reveal?: string | null;
}

const optionLabel = (key: string, label: string) =>
  key.toLowerCase() === label.toLowerCase() ? label : `${key} - ${label}`;

/** A field's hint and the recommendation of the current use case. */
function recommendedFor(f: Field, u: UseCase | null) {
  return u ? f.recommended[u] : undefined;
}

// ---- editors --------------------------------------------------------------------------

const control = "h-8 rounded-ant border border-line bg-card px-2 text-fg disabled:opacity-50";

function Editor(p: {
  f: Field;
  catalog: Catalog;
  resolved: Values;
  set: Values;
  useCase: UseCase | null;
  edit: FormProps["edit"];
  onLocalError: (m: string | null) => void;
}) {
  const { f, set, resolved } = p;
  const cur = set[f.path];
  const rec = recommendedFor(f, p.useCase);
  const id = `ctl-${f.path}`;

  if (f.options.length > 0) {
    const options = p.catalog.visibleOptions(f, resolved);
    const key = p.catalog.optionKeyOf(f, cur);
    const isInt =
      cur !== undefined && key === null && f.also_integer !== null && typeof cur === "number";
    return (
      <OptionPicker
        {...p}
        id={id}
        options={options}
        selected={key ?? (isInt ? "__int" : "")}
        recommendedKey={rec ? p.catalog.optionKeyOf(f, rec.value) : null}
      />
    );
  }
  if (isBoolField(f)) {
    // Some defaults are conditional and only described in text ("true for alsa"): read the lead word.
    const textDefault = f.default_text ? /^(true|false)\b/.exec(f.default_text)?.[1] : undefined;
    const now =
      typeof cur === "boolean"
        ? cur
        : typeof f.default === "boolean"
          ? f.default
          : textDefault === "true";
    return (
      <span className="inline-flex items-center gap-2">
        <Switch id={id} label={f.key} checked={now} onChange={(v) => p.edit({ [f.path]: v }, [])} />
        <span className="text-muted">
          {cur === undefined
            ? f.default_text && typeof f.default !== "boolean"
              ? `default: ${f.default_text}`
              : "default"
            : now
              ? "on"
              : "off"}
        </span>
      </span>
    );
  }
  return <TextEditor {...p} id={id} />;
}

type AlsaReport = AlsaProbeReport;

/** The cards whose mixer elements are offered: the card of the configured device, else all cards. */
function mixerScope(
  report: AlsaReport,
  device: unknown,
): { cards: AlsaReport["cards"]; label: string } {
  const d = typeof device === "string" ? device : "";
  const m = /^hw:(?:CARD=)?([^,]+)/.exec(d);
  const hit = m ? report.cards.filter((c) => c.id === m[1] || String(c.index) === m[1]) : [];
  const first = hit[0];
  return first ? { cards: hit, label: ` on ${first.id}` } : { cards: report.cards, label: "" };
}

function mixerItems(report: AlsaReport, device: unknown) {
  const { cards } = mixerScope(report, device);
  return cards.flatMap((c) =>
    (c.mixer_controls ?? []).map((m) => ({
      value: m.name,
      text: [
        m.has_volume && "volume",
        m.has_switch && "mute switch",
        cards.length > 1 && c.longname,
      ]
        .filter(Boolean)
        .join(", "),
    })),
  );
}

function OptionPicker(p: {
  f: Field;
  id: string;
  options: Field["options"];
  selected: string;
  recommendedKey: string | null;
  useCase: UseCase | null;
  edit: FormProps["edit"];
  onLocalError: (m: string | null) => void;
  catalog: Catalog;
  set: Values;
}) {
  const { f } = p;
  const defKey = p.catalog.optionKeyOf(f, f.default ?? undefined);
  const defLabel = f.options.find((o) => o.key === defKey)?.label;
  const [num, setNum] = useState(typeof p.set[f.path] === "number" ? String(p.set[f.path]) : "");
  const intMode = p.selected === "__int";
  const [pickInt, setPickInt] = useState(false);
  const numRef = useRef<HTMLInputElement>(null);
  useEffect(() => {
    if (!intMode) setPickInt(false);
  }, [intMode]);
  // Choosing the number entry from the list moves focus to the number input.
  useEffect(() => {
    if (pickInt && !intMode) numRef.current?.focus();
  }, [pickInt, intMode]);
  const showNum = f.also_integer !== null && (intMode || pickInt);

  const commitNum = () => {
    const n = f.also_integer;
    if (!n) return;
    const t = num.trim();
    if (!/^-?\d+$/.test(t)) return p.onLocalError("Enter a whole number.");
    const v = Number(t);
    const msg = checkRange(n.range, v);
    if (msg) return p.onLocalError(msg);
    p.onLocalError(null);
    void p.edit({ [f.path]: v }, []);
  };

  return (
    <span className="inline-flex flex-wrap items-center gap-2">
      <select
        id={p.id}
        aria-label={f.key}
        className={`${control} max-w-full`}
        value={pickInt ? "__int" : p.selected}
        onChange={(e) => {
          const v = e.target.value;
          p.onLocalError(null);
          if (v === "") void p.edit({}, [f.path]);
          else if (v === "__int") {
            setPickInt(true);
          } else {
            setPickInt(false);
            void p.edit({ [f.path]: optionValue(v) }, []);
          }
        }}
      >
        <option value="">{defLabel ? `Default (${defLabel})` : "Not set"}</option>
        {p.options.map((o) => (
          <option key={o.key} value={o.key}>
            {optionLabel(o.key, o.label)}
            {p.recommendedKey === o.key && p.useCase
              ? ` (Recommended for ${labelOfUseCase(p.useCase)})`
              : ""}
          </option>
        ))}
        {f.also_integer && <option value="__int">{f.also_integer.label}</option>}
      </select>
      {showNum && f.also_integer && (
        <input
          ref={numRef}
          type="number"
          aria-label={f.also_integer.label}
          className={`${control} w-32 font-mono`}
          value={num}
          min={f.also_integer.range[0]}
          max={f.also_integer.range[1]}
          onChange={(e) => setNum(e.target.value)}
          onBlur={commitNum}
          onKeyDown={(e) => e.key === "Enter" && commitNum()}
        />
      )}
    </span>
  );
}

function TextEditor(p: {
  f: Field;
  id: string;
  set: Values;
  edit: FormProps["edit"];
  onLocalError: (m: string | null) => void;
}) {
  const { f } = p;
  const stored = inputText(p.set[f.path]);
  const [text, setText] = useState(stored);
  useEffect(() => setText(stored), [stored]);

  const commit = () => {
    const t = text.trim();
    if (t === stored) return p.onLocalError(null);
    if (t === "") {
      p.onLocalError(null);
      return void p.edit({}, [f.path]);
    }
    const v = parseInput(t);
    const msg = checkRange(f.range, v);
    if (msg) return p.onLocalError(msg);
    p.onLocalError(null);
    void p.edit({ [f.path]: v }, []);
  };

  return (
    <input
      id={p.id}
      aria-label={f.key}
      className={`${control} w-56 max-w-full font-mono`}
      value={text}
      placeholder={f.default !== null ? showValue(f.default) : (f.default_text ?? "")}
      onChange={(e) => setText(e.target.value)}
      onBlur={commit}
      onKeyDown={(e) => e.key === "Enter" && commit()}
    />
  );
}

// ---- assists --------------------------------------------------------------------------

function Assists(p: {
  f: Field;
  catalog: Catalog;
  resolved: Values;
  edit: FormProps["edit"];
  refresh: () => void;
}) {
  const { f } = p;
  const [mixer, setMixer] = useState(false);
  const [alsa, setAlsa] = useState<{ report: AlsaProbeReport | null; error: string | null } | null>(
    null,
  );
  const [pw, setPw] = useState<{ report: PipeWireProbeReport | null; error: string | null } | null>(
    null,
  );
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);

  const probeAlsa = async (what: "list" | "check" | "mixer") => {
    setBusy(true);
    setNote(null);
    try {
      const r = await api.probeAlsa(); // every press probes again
      setAlsa(what === "check" ? null : r);
      setMixer(what === "mixer");
      if (what === "check")
        setNote(
          r.error ? `ALSA probe failed: ${r.error}` : "Checked against the detected devices.",
        );
      p.refresh();
    } finally {
      setBusy(false);
    }
  };
  const probePw = async () => {
    setBusy(true);
    try {
      setPw(await api.probePipewire());
    } finally {
      setBusy(false);
    }
  };

  const nodes: ReactNode[] = [];
  const seen = new Set<string>();
  for (const id of f.assist) {
    const a = p.catalog.assist(id);
    if (!a || seen.has(id)) continue;
    seen.add(id);
    if (a.kind === "preset") {
      const now = p.catalog.matchingPreset(id, p.resolved);
      nodes.push(
        <div key={id} className="flex flex-wrap items-center gap-2">
          <span className="text-muted">{a.label}:</span>
          {a.presets.map((pr) => (
            <Btn
              key={pr.key}
              title={pr.summary}
              onClick={() => void p.edit(pr.values as Values, [])}
              primary={now === pr.label}
            >
              {pr.label}
            </Btn>
          ))}
          <Tag tone={now ? "primary" : "default"}>now: {now ?? "Custom"}</Tag>
        </div>,
      );
    } else if (id === "alsa-detect" && f.path === "output.device") {
      nodes.push(
        <Btn key={id} onClick={() => void probeAlsa("list")} disabled={busy}>
          <Wand2 size={14} aria-hidden /> {a.label}
        </Btn>,
      );
    } else if (id === "alsa-detect" && f.path === "output.mixer_control") {
      nodes.push(
        <Btn key={id} onClick={() => void probeAlsa("mixer")} disabled={busy}>
          <Wand2 size={14} aria-hidden /> List mixer controls
        </Btn>,
      );
    } else if (id === "pipewire-sinks" && f.path === "output.target_node") {
      nodes.push(
        <Btn key={id} onClick={() => void probePw()} disabled={busy}>
          <Wand2 size={14} aria-hidden /> {a.label}
        </Btn>,
      );
    } else if (id === "alsa-detect" || id === "capability-check") {
      if (!seen.has("check-shown")) {
        seen.add("check-shown");
        nodes.push(
          <Btn key={id} onClick={() => void probeAlsa("check")} disabled={busy} title={a.label}>
            <Wand2 size={14} aria-hidden /> Check against detected device
          </Btn>,
        );
      }
    }
  }
  if (nodes.length === 0) return null;

  return (
    <div className="mt-2 space-y-2">
      <div className="flex flex-wrap items-center gap-2">{nodes}</div>
      {note && <p className="text-muted">{note}</p>}
      {alsa?.error && (
        <Alert kind="warning" label="ALSA probe failed:">
          {alsa.error}
        </Alert>
      )}
      {alsa?.report && mixer && (
        <DevicePick
          title={`Mixer controls${mixerScope(alsa.report, p.resolved["output.device"]).label}`}
          empty={mixerItems(alsa.report, p.resolved["output.device"]).length === 0}
          items={mixerItems(alsa.report, p.resolved["output.device"])}
          onPick={(v) => {
            setAlsa(null);
            void p.edit({ [f.path]: v }, []);
          }}
          onClose={() => setAlsa(null)}
        />
      )}
      {alsa?.report && !mixer && (
        <DevicePick
          title="ALSA devices"
          empty={alsa.report.cards.every((c) => c.devices.length === 0)}
          items={alsa.report.cards.flatMap((c) =>
            c.devices.map((d) => ({
              value: d.device_path,
              text: `${c.longname} / ${d.name}${d.is_busy ? " (busy)" : ""}`,
            })),
          )}
          onPick={(v) => {
            setAlsa(null);
            void p.edit({ [f.path]: v }, []);
          }}
          onClose={() => setAlsa(null)}
        />
      )}
      {pw?.error && (
        <Alert kind="warning" label="PipeWire probe failed:">
          {pw.error}
        </Alert>
      )}
      {pw?.report && (
        <DevicePick
          title="PipeWire sinks"
          empty={pw.report.sinks.length === 0}
          items={pw.report.sinks.map((s) => ({ value: s.node_name, text: s.description ?? "" }))}
          onPick={(v) => {
            setPw(null);
            void p.edit({ [f.path]: v }, []);
          }}
          onClose={() => setPw(null)}
        />
      )}
    </div>
  );
}

function DevicePick(p: {
  title: string;
  empty: boolean;
  items: { value: string; text: string }[];
  onPick: (v: string) => void;
  onClose: () => void;
}) {
  return (
    <div className="rounded-ant border border-line bg-card">
      <div className="flex items-center justify-between border-b border-line-soft px-3 py-1.5">
        <strong>{p.title}</strong>
        <button type="button" className="text-primary" onClick={p.onClose}>
          Close
        </button>
      </div>
      {p.empty ? (
        <p className="px-3 py-2 text-muted">The probe found no devices.</p>
      ) : (
        <ul>
          {p.items.map((it) => (
            <li key={it.value}>
              <button
                type="button"
                onClick={() => p.onPick(it.value)}
                className="block w-full px-3 py-1.5 text-left hover:bg-primary-bg"
              >
                <span className="font-mono">{it.value}</span>
                {it.text && <span className="ml-2 text-muted">{it.text}</span>}
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

// ---- rows -----------------------------------------------------------------------------

function Details({
  f,
  resolved,
  useCase,
  catalog,
  edit,
}: {
  f: Field;
  resolved: Values;
  useCase: UseCase | null;
  catalog: Catalog;
  edit: FormProps["edit"];
}) {
  const rec = recommendedFor(f, useCase);
  const options = catalog.visibleOptions(f, resolved);
  const selected = catalog.optionKeyOf(f, resolved[f.path]);
  return (
    <div className="mt-2 space-y-3 border-t border-line-soft pt-2">
      <Markdown text={f.description} />
      {options.length > 0 && (
        <ul className="space-y-2">
          {options.map((o) => (
            <li key={o.key}>
              {/* biome-ignore lint/a11y/useKeyWithClickEvents: outer card click; button handles keyboard */}
              {/* biome-ignore lint/a11y/noStaticElementInteractions: outer card click; button handles keyboard */}
              <div
                onClick={(e) => {
                  const target = e.target as HTMLElement;
                  if (target.closest("a, button")) return;
                  if (window.getSelection()?.toString()) return;
                  if (o.key !== selected) void edit({ [f.path]: optionValue(o.key) }, []);
                }}
                className={`cursor-pointer rounded-ant border px-3 py-2 hover:bg-primary-bg has-focus-visible:ring-2 has-focus-visible:ring-primary ${o.key === selected ? "border-primary" : "border-line-soft"}`}
              >
                <button
                  type="button"
                  aria-pressed={o.key === selected}
                  title={`Select ${o.key}`}
                  onClick={(e) => {
                    e.stopPropagation();
                    if (o.key !== selected) void edit({ [f.path]: optionValue(o.key) }, []);
                  }}
                  className="block cursor-pointer text-left outline-none"
                >
                  <code className="font-mono font-semibold">{o.key}</code>
                  {o.label.toLowerCase() !== o.key.toLowerCase() && <span> – {o.label}</span>}
                  {o.key === selected && (
                    <span className="ml-2">
                      <Tag tone="primary">selected</Tag>
                    </span>
                  )}
                </button>
                <div className="text-muted">
                  <Markdown text={o.summary} />
                </div>
                {o.description && o.description !== o.summary && <Markdown text={o.description} />}
                {o.caution && (
                  <div className="mt-1">
                    <Alert kind="warning" label="Caution:">
                      <Markdown text={o.caution} />
                    </Alert>
                  </div>
                )}
              </div>
            </li>
          ))}
          {f.also_integer && (
            <li className="rounded-ant border border-line-soft px-3 py-2">
              <div className="font-semibold">{f.also_integer.label}</div>
              <Markdown text={f.also_integer.summary} />
              {f.also_integer.description && <Markdown text={f.also_integer.description} />}
            </li>
          )}
        </ul>
      )}
      {rec && (
        <Alert
          kind="success"
          label={`Recommended for ${useCase ? labelOfUseCase(useCase) : ""}: ${showValue(rec.value)}.`}
        >
          <Markdown text={rec.why} />
        </Alert>
      )}
      <p className="text-muted">{inputHint(f)}</p>
    </div>
  );
}

function FieldRow(p: {
  f: Field;
  depth: number;
  hidden: string | null;
  required: boolean;
  expanded: boolean;
  onExpand: (open: boolean) => void;
  fp: FormProps;
  resolved: Values;
  issues: Issue[];
  findings: CapabilityFinding[];
}) {
  const { f, fp } = p;
  const cur = fp.set[f.path];
  const changed = fp.touched.has(f.path);
  const rec = recommendedFor(f, fp.useCase);
  const [local, setLocal] = useState<string | null>(null);
  const serverErr = fp.fieldErrors[f.path];
  const problems = [
    ...p.issues.map((i) => ({ k: i.error ? "error" : "warning", m: i.message })),
    ...p.findings.map((x) => ({ k: x.severity, m: x.message })),
  ] as {
    k: "error" | "warning" | "info";
    m: string;
  }[];

  return (
    <li
      id={`field-${f.path}`}
      style={{ marginLeft: `${p.depth * 16}px` }}
      className={`rounded-ant border px-3 py-2 ${p.hidden ? "opacity-60" : ""} ${
        p.expanded ? "border-primary" : "border-line-soft"
      }`}
      onFocus={(e) => {
        // The header button toggles on click; letting its own focus (which comes first on a
        // mouse press) expand the row would make the click collapse it again.
        if (e.target instanceof HTMLElement && e.target.dataset.rowToggle !== undefined) return;
        if (!p.expanded) p.onExpand(true);
      }}
    >
      <div className="flex flex-wrap items-start gap-x-4 gap-y-1">
        <div className="min-w-0 flex-1 basis-60">
          <button
            type="button"
            data-row-toggle
            aria-expanded={p.expanded}
            onClick={() => p.onExpand(!p.expanded)}
            className="flex w-full items-center gap-1 text-left"
          >
            {p.expanded ? (
              <ChevronDown size={14} aria-hidden />
            ) : (
              <ChevronRight size={14} aria-hidden />
            )}
            <code className="font-mono font-semibold">{f.key}</code>
          </button>
          <div className="pl-5 text-muted">{f.summary}</div>
          <div className="flex flex-wrap items-center gap-1 pl-5">
            {changed && <Tag tone="primary">changed</Tag>}
            {p.required && <Tag tone="danger">required</Tag>}
            {rec && <Tag tone="success">recommended: {showValue(rec.value)}</Tag>}
            {p.hidden && <Tag title={p.hidden}>hidden: {p.hidden}</Tag>}
            {(f.default !== null || f.default_text) && (
              <span className="text-xs text-muted">
                default:{" "}
                <code className="font-mono">
                  {f.default !== null ? showValue(f.default) : f.default_text}
                </code>
              </span>
            )}
          </div>
        </div>
        <div className="flex items-center gap-2">
          <Editor
            f={f}
            catalog={fp.catalog}
            resolved={p.resolved}
            set={fp.set}
            useCase={fp.useCase}
            edit={fp.edit}
            onLocalError={setLocal}
          />
          {cur !== undefined && (
            <button
              type="button"
              title="Reset to the default"
              aria-label={`Reset ${f.key}`}
              className="rounded-ant border border-line p-1.5 text-muted hover:text-primary"
              onClick={() => {
                setLocal(null);
                void fp.edit({}, [f.path]);
              }}
            >
              <RotateCcw size={14} aria-hidden />
            </button>
          )}
        </div>
      </div>
      {(local ?? serverErr) && (
        <div className="mt-2">
          <Alert kind="error">{local ?? serverErr}</Alert>
        </div>
      )}
      {problems.length > 0 && (
        <div className="mt-2 space-y-1">
          {keyed(problems, (x) => `${x.k}:${x.m}`).map(({ key, item: x }) => (
            <Alert
              key={key}
              kind={x.k === "error" ? "error" : x.k === "warning" ? "warning" : "info"}
            >
              {x.m}
            </Alert>
          ))}
        </div>
      )}
      <Assists
        f={f}
        catalog={fp.catalog}
        resolved={p.resolved}
        edit={fp.edit}
        refresh={fp.refresh}
      />
      {p.expanded && (
        <Details
          f={f}
          resolved={p.resolved}
          useCase={fp.useCase}
          catalog={fp.catalog}
          edit={fp.edit}
        />
      )}
    </li>
  );
}

// ---- the accordion --------------------------------------------------------------------

export function Form(fp: FormProps) {
  const { catalog } = fp;
  const resolved = useMemo(() => catalog.resolved(fp.set), [catalog, fp.set]);
  const searching = fp.search.trim() !== "";
  const [open, setOpen] = useState<Record<string, boolean>>(() =>
    Object.fromEntries(catalog.sections.map((s) => [s.name, !s.advanced])),
  );
  const [expanded, setExpanded] = useState<string | null>(null);
  useEffect(() => {
    const r = fp.reveal;
    if (!r) return;
    setOpen((o) => ({ ...o, [r]: true }));
    setTimeout(() => document.getElementById(`sec-${r}`)?.scrollIntoView({ block: "start" }), 50);
  }, [fp.reveal]);

  // Per section: the rows to show.
  const view = catalog.sections.map((s) => {
    const rows = s.fields
      .map((f) => ({ f, hidden: catalog.hiddenReason(f, fp.mode, resolved) }))
      .filter((r) => (fp.showHidden || r.hidden === null) && matchesSearch(r.f, fp.search));
    const secIssues = fp.issues.filter((i) => sectionOf(i.path) === s.name);
    const errors = secIssues.filter((i) => i.error).length;
    const changed = [...fp.touched].filter((t) => sectionOf(t) === s.name).length;
    return { s, rows, errors, changed };
  });

  // A section with an error opens by itself, once, when the error appears.
  const errorSections = view
    .filter((v) => v.errors > 0)
    .map((v) => v.s.name)
    .join(",");
  useEffect(() => {
    if (!errorSections) return;
    setOpen((o) => ({
      ...o,
      ...Object.fromEntries(errorSections.split(",").map((n) => [n, true])),
    }));
  }, [errorSections]);

  const visibleSections = view.filter((v) => v.rows.length > 0);
  if (visibleSections.length === 0) {
    return (
      <Alert kind="info">
        {searching ? (
          <>
            <SearchIcon size={14} className="mr-1 inline" aria-hidden />
            No key matches “{fp.search}”.
          </>
        ) : (
          "No keys to show in this mode."
        )}
      </Alert>
    );
  }

  return (
    <>
      <fieldset disabled={fp.locked !== null} className="space-y-3">
        {fp.locked && <Alert kind="info">{fp.locked}</Alert>}
        {visibleSections.map(({ s, rows, errors, changed }) => {
          const isOpen = searching ? true : (open[s.name] ?? false);
          return (
            <section key={s.name} className="rounded-ant border border-line-soft bg-card">
              <h2>
                <button
                  type="button"
                  aria-expanded={isOpen}
                  aria-controls={`sec-${s.name}`}
                  onClick={() => setOpen({ ...open, [s.name]: !isOpen })}
                  className="flex w-full flex-wrap items-center gap-2 px-4 py-3 text-left"
                >
                  {isOpen ? (
                    <ChevronDown size={16} aria-hidden />
                  ) : (
                    <ChevronRight size={16} aria-hidden />
                  )}
                  <span className="font-semibold">{s.title}</span>
                  <span className="mr-auto text-muted">{s.summary}</span>
                  {errors > 0 && (
                    <Tag tone="danger">
                      {errors} {errors === 1 ? "error" : "errors"}
                    </Tag>
                  )}
                  {changed > 0 && <Tag tone="primary">{changed} changed</Tag>}
                </button>
              </h2>
              {isOpen && (
                <div id={`sec-${s.name}`} className="space-y-2 border-t border-line-soft p-3">
                  <Markdown text={s.description} />
                  <ul className="space-y-2">
                    {rows.map(({ f, hidden }) => {
                      const depth = f.path.split(".").length - 2;
                      if (f.kind === "group") {
                        return (
                          <li
                            key={f.path}
                            style={{ marginLeft: `${depth * 16}px` }}
                            className="pt-2"
                          >
                            <h3 className="font-semibold">
                              <code className="font-mono">{f.key}</code>{" "}
                              <span className="font-normal text-muted">{f.summary}</span>
                            </h3>
                          </li>
                        );
                      }
                      return (
                        <FieldRow
                          key={f.path}
                          f={f}
                          depth={depth}
                          hidden={hidden}
                          required={catalog.isRequired(f, fp.mode, resolved)}
                          expanded={expanded === f.path}
                          onExpand={(o) => setExpanded(o ? f.path : null)}
                          fp={fp}
                          resolved={resolved}
                          issues={fp.issues.filter((i) => i.path === f.path)}
                          findings={fp.findings.filter((x) => x.path === f.path)}
                        />
                      );
                    })}
                  </ul>
                </div>
              )}
            </section>
          );
        })}
      </fieldset>
      <div className="mt-6 flex flex-wrap justify-end gap-2">
        <Btn onClick={fp.onReview}>Review</Btn>
        <Btn primary onClick={fp.onSave}>
          Save
        </Btn>
      </div>
    </>
  );
}

export type { ConfigValue };
