import { Download, FolderOpen, Menu, Moon, Power, Search, Sun } from "lucide-react";
import { type ReactNode, useState } from "react";
import type { UseCase } from "../generated/types";
import { type Mode, USE_CASES } from "../lib/catalog";

export type { Mode };

export function Btn({
  children,
  onClick,
  primary,
  danger,
  disabled,
  title,
}: {
  children: ReactNode;
  onClick?: () => void;
  primary?: boolean;
  danger?: boolean;
  disabled?: boolean;
  title?: string;
}) {
  const tone = primary
    ? "border-primary bg-primary text-white hover:opacity-90"
    : danger
      ? "border-danger text-danger hover:bg-danger-bg"
      : "border-line bg-card text-fg hover:border-primary hover:text-primary";
  return (
    <button
      type="button"
      title={title}
      disabled={disabled}
      onClick={onClick}
      className={`inline-flex h-8 items-center gap-1.5 rounded-ant border px-3 whitespace-nowrap disabled:cursor-not-allowed disabled:opacity-50 ${tone}`}
    >
      {children}
    </button>
  );
}

function Shell({
  width,
  children,
  sub,
}: {
  width: string;
  children: ReactNode;
  /** Optional second row (the form's view controls). */
  sub?: ReactNode;
}) {
  return (
    <header className="sticky top-0 z-10 border-b border-line-soft bg-card">
      <div className={`mx-auto flex flex-wrap items-center gap-2 px-4 py-2 ${width}`}>
        {children}
      </div>
      {sub}
    </header>
  );
}

export function ThemeButton({ dark, onToggle }: { dark: boolean; onToggle: () => void }) {
  return (
    <Btn onClick={onToggle} title={dark ? "Switch to light theme" : "Switch to dark theme"}>
      {dark ? <Sun size={16} aria-hidden /> : <Moon size={16} aria-hidden />}
      <span className="sr-only">Toggle theme</span>
    </Btn>
  );
}

/** Slim header of the welcome screen: title, theme toggle and Quit. */
export function WelcomeHeader(p: {
  dark: boolean;
  onTheme: () => void;
  onQuit: () => void;
  stopped: boolean;
}) {
  return (
    <Shell width="max-w-3xl">
      <h1 className="mr-auto text-base font-semibold">Audigma configurator</h1>
      <ThemeButton dark={p.dark} onToggle={p.onTheme} />
      <Btn danger onClick={p.onQuit} disabled={p.stopped}>
        <Power size={16} aria-hidden /> Quit
      </Btn>
    </Shell>
  );
}

/** Navbar of guided setup: progress, Back, Next, Skip to form. */
export function GuidedNavbar(p: {
  progress: string;
  canBack: boolean;
  canNext: boolean;
  disabled: boolean;
  onBack: () => void;
  onNext: () => void;
  onSkip: () => void;
  dark: boolean;
  onTheme: () => void;
  onQuit: () => void;
}) {
  return (
    <Shell width="max-w-3xl">
      <h1 className="text-base font-semibold">Guided setup</h1>
      <span className="mr-auto text-muted">{p.progress}</span>
      <Btn onClick={p.onBack} disabled={p.disabled || !p.canBack}>
        Back
      </Btn>
      <Btn primary onClick={p.onNext} disabled={p.disabled || !p.canNext}>
        Next
      </Btn>
      <Btn onClick={p.onSkip} disabled={p.disabled}>
        Skip to form
      </Btn>
      <ThemeButton dark={p.dark} onToggle={p.onTheme} />
      <Btn danger onClick={p.onQuit} disabled={p.disabled}>
        <Power size={16} aria-hidden /> Quit
      </Btn>
    </Shell>
  );
}

export interface FormNavbarProps {
  /** The view controls (row 2) belong to the form page only. */
  showView: boolean;
  useCase: UseCase | null;
  onUseCase: (u: UseCase | null) => void;
  mode: Mode;
  onMode: (m: Mode) => void;
  showHidden: boolean;
  onShowHidden: (v: boolean) => void;
  search: string;
  onSearch: (s: string) => void;
  errors: number;
  warnings: number;
  onSummary: () => void;
  onReview: () => void;
  onSave: () => void;
  onDownload: () => void;
  onLoad: () => void;
  canDownload: boolean;
  disabled: boolean;
  dark: boolean;
  onTheme: () => void;
  onQuit: () => void;
}

/** Navbar of the form, review and save pages. Row 1: the file (summary and actions). Row 2: the form's view controls. */
export function FormNavbar(p: FormNavbarProps) {
  const [menu, setMenu] = useState(false);
  const field = "h-8 rounded-ant border border-line bg-card px-2 text-fg";
  const view = (
    <div className="border-t border-line-soft">
      <div
        className={`${menu ? "flex" : "hidden"} mx-auto max-w-6xl flex-wrap items-center gap-2 px-4 py-2 md:flex`}
      >
        <select
          aria-label="Use case"
          className={field}
          value={p.useCase ?? ""}
          disabled={p.disabled}
          onChange={(e) => p.onUseCase((e.target.value || null) as UseCase | null)}
        >
          <option value="">No use case</option>
          {USE_CASES.map((u) => (
            <option key={u.id} value={u.id}>
              {u.label}
            </option>
          ))}
        </select>
        <select
          aria-label="Mode"
          className={field}
          value={p.mode}
          onChange={(e) => p.onMode(e.target.value as Mode)}
        >
          <option value="all">All keys</option>
          <option value="live">Live</option>
          <option value="offline">Offline</option>
        </select>
        <label className="inline-flex items-center gap-1.5 text-xs text-muted whitespace-nowrap">
          <input
            type="checkbox"
            checked={p.showHidden}
            onChange={(e) => p.onShowHidden(e.target.checked)}
          />
          Show hidden keys
        </label>
        <label className="relative ml-auto w-full min-w-40 sm:w-64">
          <span className="sr-only">Search keys</span>
          <Search size={14} aria-hidden className="absolute top-2.5 left-2 text-muted" />
          <input
            type="search"
            placeholder="Search"
            className={`${field} w-full pl-7`}
            value={p.search}
            onChange={(e) => p.onSearch(e.target.value)}
          />
        </label>
      </div>
    </div>
  );
  return (
    <Shell width="max-w-6xl" sub={p.showView ? view : undefined}>
      <h1 className="mr-2 text-base font-semibold">Audigma</h1>
      {p.showView && (
        <button
          type="button"
          aria-label="View options"
          aria-expanded={menu}
          className="rounded-ant border border-line p-1.5 md:hidden"
          onClick={() => setMenu(!menu)}
        >
          <Menu size={16} aria-hidden />
        </button>
      )}
      <button
        type="button"
        onClick={p.onSummary}
        className="text-left"
        title="Jump to the first problem"
      >
        <span className={p.errors ? "text-danger" : "text-muted"}>
          {p.errors} {p.errors === 1 ? "error" : "errors"}
        </span>
        {", "}
        <span className={p.warnings ? "text-warning" : "text-muted"}>
          {p.warnings} {p.warnings === 1 ? "warning" : "warnings"}
        </span>
      </button>
      <div className="ml-auto flex flex-wrap items-center gap-2">
        <Btn onClick={p.onReview} disabled={p.disabled}>
          Review
        </Btn>
        <Btn primary onClick={p.onSave} disabled={p.disabled}>
          Save
        </Btn>
        <Btn
          onClick={p.onDownload}
          disabled={p.disabled || !p.canDownload}
          title={p.canDownload ? "Download the file" : "Fix the errors first"}
        >
          <Download size={16} aria-hidden />
          <span className="hidden sm:inline">Download</span>
          <span className="sr-only sm:hidden">Download</span>
        </Btn>
        <Btn onClick={p.onLoad} disabled={p.disabled} title="Load a file">
          <FolderOpen size={16} aria-hidden />
          <span className="hidden sm:inline">Load file…</span>
          <span className="sr-only sm:hidden">Load file</span>
        </Btn>
        <ThemeButton dark={p.dark} onToggle={p.onTheme} />
        <span className="mx-1 hidden h-5 border-l border-line-soft sm:block" aria-hidden />
        <Btn danger onClick={p.onQuit} disabled={p.disabled} title="Quit the configurator">
          <Power size={16} aria-hidden />
          <span className="hidden sm:inline">Quit</span>
          <span className="sr-only sm:hidden">Quit</span>
        </Btn>
      </div>
    </Shell>
  );
}
