import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { call } from "./client";
import type { AlbumSummary } from "../bindings/AlbumSummary";
import type { MonthCount } from "../bindings/MonthCount";
import type { PhotoAsset } from "../bindings/PhotoAsset";
import type { PhotoCursor } from "../bindings/PhotoCursor";
import type { PhotoFilter } from "../bindings/PhotoFilter";
import type { PhotoPage } from "../bindings/PhotoPage";

/** The default filter: what the grid shows with no toolbar filter active. */
export const DEFAULT_PHOTO_FILTER: PhotoFilter = {
  kind: null,
  favoritesOnly: false,
  screenshotsOnly: false,
  trashedOnly: false,
  hiddenOnly: false,
  albumId: null,
};

/** The camera roll, newest first, loading older pages on demand. */
export function usePhotos(handle: string | undefined, filter: PhotoFilter) {
  return useInfiniteQuery({
    queryKey: ["photos", "list", handle, filter],
    enabled: Boolean(handle),
    initialPageParam: null as PhotoCursor | null,
    queryFn: ({ pageParam }) =>
      call<PhotoPage>("photos_list", { handle, filter, cursor: pageParam }),
    getNextPageParam: (last) => last.older ?? undefined,
  });
}

/** Per-month counts, for the grid's headers and year jump. */
export function usePhotoTimeline(handle: string | undefined, filter: PhotoFilter) {
  return useQuery({
    queryKey: ["photos", "timeline", handle, filter],
    queryFn: () => call<MonthCount[]>("photos_timeline", { handle, filter }),
    enabled: Boolean(handle),
  });
}

/** Titled albums with their counts. */
export function usePhotoAlbums(handle: string | undefined) {
  return useQuery({
    queryKey: ["photos", "albums", handle],
    queryFn: () => call<AlbumSummary[]>("photos_albums", { handle }),
    enabled: Boolean(handle),
  });
}

/** One asset, for the lightbox. */
export function usePhotoDetail(handle: string | undefined, id: string | null) {
  return useQuery({
    queryKey: ["photos", "detail", handle, id],
    queryFn: () => call<PhotoAsset>("photo_detail", { handle, id }),
    enabled: Boolean(handle) && Boolean(id),
  });
}
