import { useCallback, useEffect, useRef, useState } from "react";
import type { Stage } from "../generated/types";
import { ApiFailure, api, followJob } from "./client";

export type BenchState =
  | { state: "idle" }
  | { state: "running"; id: number; stage: Stage | null; fraction: number }
  | { state: "failed"; message: string }
  | { state: "cancelled" };

/**
 * "Validate on this machine": starts the job, follows its events and calls `onDone` when it ended
 * (the result itself comes back in the draft view's `validation`).
 */
export function useBenchmark(onDone: () => void) {
  const [bench, setBench] = useState<BenchState>({ state: "idle" });
  const close = useRef<(() => void) | null>(null);
  useEffect(() => () => close.current?.(), []);

  const follow = useCallback(
    (id: number) => {
      close.current?.();
      setBench({ state: "running", id, stage: null, fraction: 0 });
      close.current = followJob(
        id,
        (e) => {
          if (e.type === "progress")
            setBench({ state: "running", id, stage: e.stage, fraction: e.fraction });
          else if (e.type === "finished") {
            setBench({ state: "idle" });
            onDone();
          } else if (e.type === "failed") {
            setBench({ state: "failed", message: e.message });
            onDone();
          } else if (e.type === "cancelled") {
            setBench({ state: "cancelled" });
            onDone();
          }
        },
        () => {
          setBench({ state: "failed", message: "Lost the connection to the benchmark." });
          onDone();
        },
      );
    },
    [onDone],
  );

  const start = useCallback(async () => {
    try {
      const { id } = await api.startBenchmark();
      follow(id);
    } catch (e) {
      setBench({ state: "failed", message: e instanceof ApiFailure ? e.message : String(e) });
    }
  }, [follow]);

  const cancel = useCallback(() => {
    if (bench.state === "running") void api.cancelBenchmark(bench.id).catch(() => {});
  }, [bench]);

  return { bench, start, cancel };
}
