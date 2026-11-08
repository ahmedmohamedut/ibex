import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { call } from "./client";
import type { IndexStatus } from "../bindings/IndexStatus";
import type { Paged } from "../bindings/Paged";
import type { SearchFilters } from "../bindings/SearchFilters";
import type { SearchHit } from "../bindings/SearchHit";

/** Whether a usable search index already exists for this backup. */
export function useIndexStatus(handle: string | undefined) {
  return useQuery({
    queryKey: ["search", "status", handle],
    queryFn: () => call<IndexStatus>("index_status", { handle }),
    enabled: Boolean(handle),
  });
}

/** Start building the search index. Resolves to the job id at once. */
export function useStartIndex() {
  const queryClient = useQueryClient();
  return useMutation({
    mutationFn: (handle: string) => call<string>("start_index", { handle }),
    onSuccess: () => {
      void queryClient.invalidateQueries({ queryKey: ["jobs"] });
    },
  });
}

const EMPTY_FILTERS: SearchFilters = { source: null, conversationId: null };

/** Search the built index. Disabled until there is something to search for. */
export function useSearch(
  handle: string | undefined,
  query: string,
  filters: SearchFilters = EMPTY_FILTERS,
) {
  const trimmed = query.trim();
  return useQuery({
    queryKey: ["search", "query", handle, trimmed, filters],
    queryFn: () =>
      call<Paged<SearchHit>>("search", {
        handle,
        query: trimmed,
        filters,
        page: { offset: 0, limit: 50 },
      }),
    enabled: Boolean(handle) && trimmed.length > 0,
  });
}
