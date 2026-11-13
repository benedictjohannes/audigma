import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { Form } from "./components/Form";
import { GuidedStep } from "./components/Guided";
import { FormNavbar, GuidedNavbar, WelcomeHeader } from "./components/Navbar";
import { Review } from "./components/Review";
import { Commands, downloadCommands, type SaveOptions, SavePage } from "./components/Save";
import { Alert } from "./components/ui";
import { LoadDialog, UseCaseDialog, Welcome } from "./components/Welcome";
import type {
  DraftView,
  GuidedResponse,
  GuidedState,
  MetaBundle,
  SaveResponse,
  UseCase,
} from "./generated/types";
import { useBenchmark } from "./lib/bench";
import { Catalog, flatten, type Mode, modeForUseCase, type Values } from "./lib/catalog";
import { ApiFailure, api, followJob } from "./lib/client";
import { downloadText } from "./lib/download";
import { usePage } from "./lib/route";
import { applyTheme, currentTheme, storeTheme, type Theme } from "./lib/theme";

function Notice({ title, children }: { title: string; children?: React.ReactNode }) {
  return (
    <div className="rounded-ant border border-line bg-card p-4">
      <h2 className="mb-1 text-base font-semibold">{title}</h2>
      <div className="text-muted">{children}</div>
    </div>
  );
}

export function App() {
  const [theme, setTheme] = useState<Theme>(currentTheme);
  const [draft, setDraft] = useState<DraftView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [stopped, setStopped] = useState(false);
  const [page, goto] = usePage();
  const [mode, setMode] = useState<Mode>("all");
  const [showHidden, setShowHidden] = useState(false);
  const [search, setSearch] = useState("");
  const [meta, setMeta] = useState<MetaBundle | null>(null);
  const [fieldErrors, setFieldErrors] = useState<Record<string, string>>({});
  // guided setup
  const [guided, setGuided] = useState<GuidedState | null>(null);
  const [guidedSel, setGuidedSel] = useState<string | null>(null);
  const [guidedBusy, setGuidedBusy] = useState(false);
  const [discoveryJob, setDiscoveryJob] = useState<number | null>(null);
  const closeDiscovery = useRef<(() => void) | null>(null);
  const [banner, setBanner] = useState(false);
  const [reveal, setReveal] = useState<string | null>(null);
  // save, download, dialogs
  const [saveOpts, setSaveOpts] = useState<SaveOptions>({
    path: null,
    only_changed: true,
    comment_level: "brief",
  });
  const [saved, setSaved] = useState<SaveResponse | null>(null);
  const [downloaded, setDownloaded] = useState<string | null>(null);
  const [cleanSig, setCleanSig] = useState<string | null>(null);
  const [useCaseDialog, setUseCaseDialog] = useState(false);
  const [loadDialog, setLoadDialog] = useState(false);

  useEffect(() => applyTheme(theme), [theme]);
  const toggleTheme = () => {
    const next: Theme = theme === "dark" ? "light" : "dark";
    storeTheme(next);
    setTheme(next);
  };

  const fail = useCallback((e: unknown) => {
    if (e instanceof ApiFailure && e.status === 0) setStopped(true);
    setError(e instanceof Error ? e.message : String(e));
  }, []);

  useEffect(() => {
    api.draft().then((d) => {
      setDraft(d);
      setMode(modeForUseCase(d.use_case));
      if (d.guided_active) {
        // A reload in the middle of guided setup resumes it.
        api.guidedState().then((s) => {
          setGuided(s);
          const st = s.step;
          setGuidedSel(st ? (st.choices[st.selected]?.key ?? null) : null);
          goto("guided");
        }, fail);
      }
    }, fail);
    api.meta().then(setMeta, fail);
  }, [fail, goto]);
  useEffect(() => () => closeDiscovery.current?.(), []);

  const catalog = useMemo(() => (meta ? new Catalog(meta) : null), [meta]);
  const set: Values = useMemo(() => (draft ? flatten(draft.values as never) : {}), [draft]);
  const touched = useMemo(() => new Set(draft?.touched ?? []), [draft]);

  const refresh = useCallback(() => {
    api.draft().then(setDraft, fail);
  }, [fail]);
  const { bench, start, cancel } = useBenchmark(refresh);

  /** Sends one change; the response replaces the draft view. A refusal shows on its key. */
  const edit = useCallback(
    async (s: Values, unset: string[]): Promise<boolean> => {
      const keys = [...Object.keys(s), ...unset];
      try {
        const d = await api.edit({ set: s, unset });
        setDraft(d);
        setFieldErrors((e) =>
          Object.fromEntries(Object.entries(e).filter(([k]) => !keys.includes(k))),
        );
        setError(null);
        return true;
      } catch (e) {
        if (e instanceof ApiFailure && e.status === 422) {
          setFieldErrors((m) => ({ ...m, [e.path ?? keys[0] ?? ""]: e.message }));
        } else fail(e);
        return false;
      }
    },
    [fail],
  );

  const locked = draft?.guided_active
    ? "Guided setup is running; finish it or skip to the form to edit keys."
    : bench.state === "running"
      ? "A benchmark is running; edits are off until it ends."
      : null;

  // Changes that were neither saved nor downloaded.
  const sig = draft ? JSON.stringify(draft.values) : "";
  const dirty = (draft?.touched.length ?? 0) > 0 && sig !== cleanSig;
  const quit = async () => {
    if (dirty && !window.confirm("Quit without saving or downloading? Your changes will be lost."))
      return;
    try {
      await api.quit();
    } catch (e) {
      if (!(e instanceof ApiFailure && e.status === 0)) return fail(e);
    }
    setStopped(true);
  };

  const issues = draft?.issues ?? [];
  const errors = issues.filter((i) => i.error).length;
  const warnings = issues.length - errors + (draft?.findings.length ?? 0);
  const onWelcome = draft?.is_new && page === null && !draft.loaded_from && !guided?.active;
  const common = { dark: theme === "dark", onTheme: toggleTheme, onQuit: quit };

  const setUseCase = (u: UseCase | null) =>
    api.setUseCase(u).then((d) => {
      setDraft(d);
      setMode(modeForUseCase(u));
      setError(null);
    }, fail);

  const jumpToFirst = () => {
    const first = issues.find((i) => i.error && i.path) ?? issues.find((i) => i.path);
    if (!first?.path) return goto("review");
    setSearch("");
    goto("form");
    setTimeout(
      () => document.getElementById(`field-${first.path}`)?.scrollIntoView({ block: "center" }),
      50,
    );
  };

  // ---- loading a file ----------------------------------------------------------------
  /** Resolves with the error text, or `null` after the draft was replaced and the form opened. */
  const loadText = async (text: string, name?: string): Promise<string | null> => {
    try {
      const d = await api.load({ text, name: name ?? null });
      setDraft(d);
      setMode(modeForUseCase(d.use_case));
      setFieldErrors({});
      setCleanSig(null);
      setSaved(null);
      setDownloaded(null);
      setError(null);
      setLoadDialog(false);
      goto("form");
      return null;
    } catch (e) {
      if (e instanceof ApiFailure && e.status === 0) setStopped(true);
      return e instanceof Error ? e.message : String(e);
    }
  };

  // ---- guided setup ------------------------------------------------------------------
  const takeGuidedState = useCallback((s: GuidedState) => {
    setGuided(s);
    const st = s.step;
    setGuidedSel(st ? (st.choices[st.selected]?.key ?? null) : null);
  }, []);

  const followDiscovery = (id: number) => {
    closeDiscovery.current?.();
    setDiscoveryJob(id);
    const finish = () => {
      closeDiscovery.current?.();
      setDiscoveryJob(null);
      api.guidedState().then((st) => {
        takeGuidedState(st);
        // After a sweep the step stays on "discover"; its "Skip" is the way on.
        if (st.step?.kind === "discover" && st.discovery.tiers.length > 0) setGuidedSel("skip");
      }, fail);
    };
    closeDiscovery.current = followJob(
      id,
      (e) => {
        if (e.type === "discovery") setGuided((g) => (g ? { ...g, discovery: e.discovery } : g));
        else if (e.type === "progress")
          setGuided((g) =>
            g ? { ...g, discovery: { ...g.discovery, progress: e.fraction, active: true } } : g,
          );
        else finish();
      },
      finish,
    );
  };

  const applyGuided = (r: GuidedResponse) => {
    takeGuidedState(r.state);
    if (r.draft) {
      setDraft(r.draft);
      setMode(modeForUseCase(r.draft.use_case));
    }
    if (r.next === "benchmark" && r.benchmark_id !== null) followDiscovery(r.benchmark_id);
    else if (r.next === "review") {
      setBanner(true);
      goto("review");
    } else if (r.next === "form_offline") {
      setMode("offline");
      setReveal("offline");
      goto("form");
    }
  };

  const guidedCall = async (f: () => Promise<GuidedResponse>) => {
    setGuidedBusy(true);
    try {
      applyGuided(await f());
      setError(null);
    } catch (e) {
      fail(e);
    } finally {
      setGuidedBusy(false);
    }
  };
  const startGuided = () => {
    setBanner(false);
    void guidedCall(api.guidedStart).then(() => goto("guided"));
  };
  const skipGuided = async () => {
    if (!window.confirm("Skip to the form? The answers so far are kept.")) return;
    try {
      await api.guidedSkip();
      setGuided(null);
      refresh();
      goto("form");
    } catch (e) {
      fail(e);
    }
  };

  // ---- download ----------------------------------------------------------------------
  const downloadFrom = (pv: { filename: string; text: string }) => {
    downloadText(pv.filename, pv.text);
    setDownloaded(pv.filename);
    setCleanSig(sig);
  };
  const download = async () => {
    try {
      downloadFrom(await api.preview(saveOpts));
      setError(null);
    } catch (e) {
      fail(e);
    }
  };

  const guidedStepActive = guided?.active === true;
  const discovering = discoveryJob !== null || guided?.discovery.active === true;
  const step = guided?.step ?? null;

  let nav: ReactNode;
  if (onWelcome) {
    nav = <WelcomeHeader {...common} stopped={stopped} />;
  } else if (page === "guided") {
    nav = (
      <GuidedNavbar
        {...common}
        progress={step ? `Step ${step.number} of ${step.total}` : ""}
        canBack={guided?.can_go_back === true && !discovering && !guidedBusy}
        canNext={guidedSel !== null && !discovering && !guidedBusy && !step?.waiting_for_probe}
        disabled={stopped}
        onBack={() => void guidedCall(api.guidedBack)}
        onNext={() => guidedSel !== null && void guidedCall(() => api.guidedAnswer(guidedSel))}
        onSkip={() => void skipGuided()}
      />
    );
  } else {
    nav = (
      <FormNavbar
        {...common}
        showView={page !== "review" && page !== "save"}
        useCase={draft?.use_case ?? null}
        onUseCase={setUseCase}
        mode={mode}
        onMode={setMode}
        showHidden={showHidden}
        onShowHidden={setShowHidden}
        search={search}
        onSearch={setSearch}
        errors={errors}
        warnings={warnings}
        onSummary={jumpToFirst}
        onReview={() => goto("review")}
        onSave={() => goto("save")}
        onDownload={() => void download()}
        onLoad={() => setLoadDialog(true)}
        canDownload={errors === 0}
        disabled={stopped || draft === null || catalog === null}
      />
    );
  }

  const width = page === "guided" ? "max-w-3xl" : "max-w-4xl";
  return (
    <div className="min-h-screen">
      {nav}
      {stopped && !saved && (
        <div role="alert" className="border-b border-warning bg-warning-bg px-4 py-2 text-center">
          The configurator has stopped. Nothing more can be changed here; close this tab.
        </div>
      )}
      {error && (
        <div className={`mx-auto p-4 pb-0 ${width}`}>
          <Alert kind="error">{error}</Alert>
        </div>
      )}
      {onWelcome && draft ? (
        <Welcome
          path={draft.path}
          onLoad={loadText}
          onGuided={startGuided}
          onEdit={() => setUseCaseDialog(true)}
          disabled={stopped}
        />
      ) : (
        <main className={`mx-auto space-y-3 p-4 ${width}`}>
          {downloaded && page !== "save" && (
            <Alert kind="success" label={`Downloaded ${downloaded}.`}>
              Put it where you want it, then try (fill in the path):
              <div className="mt-2">
                <Commands commands={downloadCommands(downloaded)} />
              </div>
            </Alert>
          )}
          {draft && catalog ? (
            page === "guided" ? (
              guidedStepActive ? (
                <GuidedStep
                  state={guided}
                  selected={guidedSel}
                  onChoose={(k) => {
                    setGuidedSel(k);
                    void guidedCall(() => api.guidedAnswer(k));
                  }}
                  onCancelDiscovery={() =>
                    discoveryJob !== null && void api.cancelBenchmark(discoveryJob)
                  }
                  busy={guidedBusy || discovering || stopped}
                />
              ) : (
                <Notice title="Guided setup is not running">
                  <a className="text-primary underline" href="#/form">
                    Open the form
                  </a>
                </Notice>
              )
            ) : page === "review" ? (
              <Review
                draft={draft}
                set={set}
                bench={bench}
                onStart={() => void start()}
                onCancel={cancel}
                refresh={refresh}
                onForm={() => {
                  setBanner(false);
                  goto("form");
                }}
                onSave={() => goto("save")}
                banner={banner}
                locked={locked !== null}
              />
            ) : page === "save" ? (
              <SavePage
                opts={saveOpts}
                onOpts={setSaveOpts}
                hasErrors={errors > 0}
                stopped={stopped}
                saved={saved}
                onSaved={(r) => {
                  setSaved(r);
                  setCleanSig(sig);
                  setStopped(true);
                }}
                onDownload={downloadFrom}
                downloaded={downloaded}
                locked={locked !== null}
              />
            ) : (
              <>
                <p className="px-1 text-muted">
                  Editing <code className="font-mono">{draft.path}</code>
                  {draft.is_new ? " (new file)" : ""}
                  {draft.loaded_from ? ` (loaded from ${draft.loaded_from})` : ""}
                </p>
                <Form
                  catalog={catalog}
                  mode={mode}
                  showHidden={showHidden}
                  search={search}
                  useCase={draft.use_case}
                  set={set}
                  touched={touched}
                  issues={issues}
                  findings={draft.findings}
                  locked={locked}
                  fieldErrors={fieldErrors}
                  edit={edit}
                  refresh={refresh}
                  onReview={() => goto("review")}
                  onSave={() => goto("save")}
                  reveal={reveal}
                />
              </>
            )
          ) : (
            <Notice title="Loading…" />
          )}
        </main>
      )}
      {useCaseDialog && (
        <UseCaseDialog
          onPick={(u) => {
            setUseCaseDialog(false);
            void setUseCase(u).then(() => goto("form"));
          }}
          onSkip={() => {
            setUseCaseDialog(false);
            goto("form");
          }}
        />
      )}
      {loadDialog && (
        <LoadDialog onLoad={loadText} onClose={() => setLoadDialog(false)} dirty={dirty} />
      )}
    </div>
  );
}
