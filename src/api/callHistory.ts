import { useQuery } from "@tanstack/react-query";
import { call } from "./client";
import type { CallRecord } from "../bindings/CallRecord";
import type { Paged } from "../bindings/Paged";

/** Call history, newest first. */
export function useCallHistory(handle: string | undefined) {
  return useQuery({
    queryKey: ["callHistory", handle],
    queryFn: () =>
      call<Paged<CallRecord>>("call_history_list", {
        handle,
        page: { offset: 0, limit: 2000 },
      }),
    enabled: Boolean(handle),
  });
}
