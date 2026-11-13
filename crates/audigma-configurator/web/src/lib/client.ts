import type {
  AlsaProbeResponse,
  DraftEdit,
  DraftView,
  GuidedResponse,
  GuidedState,
  JobEvent,
  JobStarted,
  LoadRequest,
  MetaBundle,
  PipeWireProbeResponse,
  PreviewResponse,
  SaveRequest,
  SaveResponse,
  UseCase,
  ValidateResponse,
} from "../generated/types";

export class ApiFailure extends Error {
  constructor(
    public status: number,
    message: string,
    public path?: string | null,
  ) {
    super(message);
  }
}

async function call<T>(method: string, url: string, body?: unknown): Promise<T> {
  let res: Response;
  try {
    res = await fetch(url, {
      method,
      headers: body === undefined ? undefined : { "Content-Type": "application/json" },
      body: body === undefined ? undefined : JSON.stringify(body),
    });
  } catch {
    throw new ApiFailure(0, "The configurator is not reachable (it may have stopped).");
  }
  const text = await res.text();
  let json: unknown = null;
  try {
    json = text ? JSON.parse(text) : null;
  } catch {
    // not JSON
  }
  if (!res.ok) {
    const e = (json ?? {}) as { error?: string; path?: string | null };
    throw new ApiFailure(res.status, e.error ?? `${method} ${url}: ${res.status}`, e.path);
  }
  return json as T;
}

export const api = {
  draft: () => call<DraftView>("GET", "/api/draft"),
  edit: (e: DraftEdit) => call<DraftView>("PUT", "/api/draft", e),
  setUseCase: (use_case: UseCase | null) => call<DraftView>("PUT", "/api/draft", { use_case }),
  meta: () => call<MetaBundle>("GET", "/api/meta"),
  validate: () => call<ValidateResponse>("POST", "/api/validate"),
  probeAlsa: () => call<AlsaProbeResponse>("GET", "/api/probe/alsa"),
  probePipewire: () => call<PipeWireProbeResponse>("GET", "/api/probe/pipewire"),
  startBenchmark: () => call<JobStarted>("POST", "/api/benchmark"),
  cancelBenchmark: (id: number) => call<null>("DELETE", `/api/benchmark/${id}`),
  guidedState: () => call<GuidedState>("GET", "/api/guided/state"),
  guidedStart: () => call<GuidedResponse>("POST", "/api/guided/start"),
  guidedAnswer: (key: string) => call<GuidedResponse>("POST", "/api/guided/answer", { key }),
  guidedBack: () => call<GuidedResponse>("POST", "/api/guided/back"),
  guidedSkip: () => call<GuidedResponse>("POST", "/api/guided/skip"),
  load: (r: LoadRequest) => call<DraftView>("POST", "/api/load", r),
  preview: (r: SaveRequest) => call<PreviewResponse>("POST", "/api/preview", r),
  save: (r: SaveRequest) => call<SaveResponse>("POST", "/api/save", r),
  quit: () => call<null>("POST", "/api/quit"),
};

const JOB_EVENTS = ["progress", "discovery", "finished", "failed", "cancelled"] as const;

/**
 * Follows a job's event stream (the server replays its log on a reconnect). Returns a function
 * that closes the stream. `onLost` runs when the stream ends without a final event.
 */
export function followJob(
  id: number,
  onEvent: (e: JobEvent) => void,
  onLost: () => void,
): () => void {
  const es = new EventSource(`/api/benchmark/${id}/events`);
  let ended = false;
  for (const name of JOB_EVENTS) {
    es.addEventListener(name, (m) => {
      const e = JSON.parse((m as MessageEvent<string>).data) as JobEvent;
      if (e.type === "finished" || e.type === "failed" || e.type === "cancelled") {
        ended = true;
        es.close();
      }
      onEvent(e);
    });
  }
  es.onerror = () => {
    es.close();
    if (!ended) onLost();
  };
  return () => es.close();
}
