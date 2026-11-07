import {
  useMutation,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query";
import { call } from "./client";
import type { BackupStats } from "../bindings/BackupStats";
import type { BackupSummary } from "../bindings/BackupSummary";
import type { RecentBackup } from "../bindings/RecentBackup";

/** Open a backup folder. `password` is for M7; an encrypted backup rejects for now. */
export function useOpenBackup() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (vars: { path: string; password?: string }) =>
      call<BackupSummary>("open_backup", {
        path: vars.path,
        password: vars.password ?? null,
      }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["recents"] });
    },
  });
}

export function useCloseBackup() {
  return useMutation({
    mutationFn: (handle: string) => call<void>("close_backup", { handle }),
  });
}

export function useRecentBackups() {
  return useQuery({
    queryKey: ["recents"],
    queryFn: () => call<RecentBackup[]>("recent_backups"),
  });
}

export function useForgetRecent() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (path: string) => call<void>("forget_recent", { path }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["recents"] });
    },
  });
}

export function useBackupStats(handle: string | undefined) {
  return useQuery({
    queryKey: ["stats", handle],
    queryFn: () => call<BackupStats>("backup_stats", { handle }),
    enabled: Boolean(handle),
  });
}

/** Clear this backup's cache. Resolves to the number of bytes freed. */
export function useClearCache() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (handle: string) => call<number>("clear_cache", { handle }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["stats"] });
    },
  });
}
