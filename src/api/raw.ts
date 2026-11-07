import { useInfiniteQuery, useMutation, useQuery } from "@tanstack/react-query";
import { call } from "./client";
import type { DomainSummary } from "../bindings/DomainSummary";
import type { FileInfo } from "../bindings/FileInfo";
import type { HexPreview } from "../bindings/HexPreview";
import type { ManifestEntry } from "../bindings/ManifestEntry";
import type { Paged } from "../bindings/Paged";
import type { SaveReport } from "../bindings/SaveReport";
import type { TextPreview } from "../bindings/TextPreview";

/** How many directory entries to fetch per page. */
const PAGE_SIZE = 500;

export function useDomains(handle: string | undefined) {
  return useQuery({
    queryKey: ["domains", handle],
    queryFn: () => call<DomainSummary[]>("list_domains", { handle }),
    enabled: Boolean(handle),
  });
}

/**
 * One directory's direct children, paged.
 *
 * A domain can hold tens of thousands of entries, so pages are pulled on demand
 * and the table virtualizes what it renders.
 */
export function useDirListing(
  handle: string | undefined,
  domain: string | null,
  dir: string,
) {
  return useInfiniteQuery({
    queryKey: ["dir", handle, domain, dir],
    enabled: Boolean(handle) && Boolean(domain),
    initialPageParam: 0,
    queryFn: ({ pageParam }) =>
      call<Paged<ManifestEntry>>("list_dir", {
        handle,
        domain,
        path: dir,
        page: { offset: pageParam, limit: PAGE_SIZE },
      }),
    getNextPageParam: (last) => last.nextOffset ?? undefined,
  });
}

export function useFileInfo(handle: string | undefined, fileId: string | null) {
  return useQuery({
    queryKey: ["fileInfo", handle, fileId],
    queryFn: () => call<FileInfo>("file_info", { handle, fileId }),
    enabled: Boolean(handle) && Boolean(fileId),
  });
}

export function useTextPreview(
  handle: string | undefined,
  fileId: string | null,
  enabled: boolean,
) {
  return useQuery({
    queryKey: ["textPreview", handle, fileId],
    queryFn: () => call<TextPreview>("read_text_preview", { handle, fileId }),
    enabled: enabled && Boolean(handle) && Boolean(fileId),
  });
}

export function useHexPreview(
  handle: string | undefined,
  fileId: string | null,
  offset: number,
  enabled: boolean,
) {
  return useQuery({
    queryKey: ["hexPreview", handle, fileId, offset],
    queryFn: () =>
      call<HexPreview>("read_hex_preview", { handle, fileId, offset, len: 2048 }),
    enabled: enabled && Boolean(handle) && Boolean(fileId),
  });
}

/** Copy a file out of the backup, keeping its original modification time. */
export function useSaveFileAs() {
  return useMutation({
    mutationFn: (vars: { handle: string; fileId: string; dest: string }) =>
      call<SaveReport>("save_file_as", vars),
  });
}
