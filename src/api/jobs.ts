import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { listen } from "@tauri-apps/api/event";
import { useEffect } from "react";
import { call } from "./client";
import type { ExportFormat } from "../bindings/ExportFormat";
import type { ExportOptions } from "../bindings/ExportOptions";
import type { JobFailedEvent } from "../bindings/JobFailedEvent";
import type { JobFinishedEvent } from "../bindings/JobFinishedEvent";
import type { JobInfo } from "../bindings/JobInfo";
import type { JobProgressEvent } from "../bindings/JobProgressEvent";
import type { Selection } from "../bindings/Selection";

/** Every job this session has run, newest first. */
export function useJobs() {
  return useQuery({
    queryKey: ["jobs"],
    queryFn: () => call<JobInfo[]>("list_jobs"),
    // Jobs change through events, which invalidate this; no polling needed.
    staleTime: Infinity,
  });
}

/** Start an export. Resolves to the job id as soon as the work is queued. */
export function useStartExport() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (vars: {
      handle: string;
      selection: Selection;
      format: ExportFormat;
      outDir: string;
      options?: ExportOptions;
    }) => call<string>("start_export", vars),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["jobs"] });
    },
  });
}

export function useCancelJob() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (jobId: string) => call<boolean>("cancel_job", { jobId }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["jobs"] });
    },
  });
}

/**
 * Keep the job list in step with the backend's events.
 *
 * Progress is already throttled on the Rust side to about ten a second, so
 * refreshing the list on each one is cheap.
 */
export function useJobEvents() {
  const queryClient = useQueryClient();
  useEffect(() => {
    const refresh = () => {
      void queryClient.invalidateQueries({ queryKey: ["jobs"] });
    };
    const unlisteners = [
      listen<JobProgressEvent>("job://progress", refresh),
      listen<JobFinishedEvent>("job://finished", refresh),
      listen<JobFailedEvent>("job://failed", refresh),
    ];
    return () => {
      for (const pending of unlisteners) {
        void pending.then((off) => off());
      }
    };
  }, [queryClient]);
}
