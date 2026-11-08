import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { call } from "./client";
import type { DocSource } from "../bindings/DocSource";
import type { Page } from "../bindings/Page";
import type { Paged } from "../bindings/Paged";
import type { VoiceRecording } from "../bindings/VoiceRecording";

/** How many recordings to fetch per page. */
const PAGE_SIZE = 200;

/** Voice Memos, newest first. */
export function useVoiceRecordings(handle: string | undefined) {
  return useInfiniteQuery({
    queryKey: ["voice", "list", handle],
    enabled: Boolean(handle),
    initialPageParam: 0,
    queryFn: ({ pageParam }) =>
      call<Paged<VoiceRecording>>("voice_list", {
        handle,
        page: { offset: pageParam, limit: PAGE_SIZE } satisfies Page,
      }),
    getNextPageParam: (last) => last.nextOffset ?? undefined,
  });
}

/** The browsable Files sources: On My iPhone and per-app Documents. */
export function useDocumentSources(handle: string | undefined) {
  return useQuery({
    queryKey: ["documents", "sources", handle],
    queryFn: () => call<DocSource[]>("documents_sources", { handle }),
    enabled: Boolean(handle),
  });
}
