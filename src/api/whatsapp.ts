import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { call } from "./client";
import type { ConversationSummary } from "../bindings/ConversationSummary";
import type { MessageCursor } from "../bindings/MessageCursor";
import type { MessagePage } from "../bindings/MessagePage";
import type { Paged } from "../bindings/Paged";

/**
 * WhatsApp chats, most recently active first.
 *
 * `includeBroadcasts` reveals status, channel and broadcast sessions, which are
 * feeds rather than conversations and are hidden by default.
 */
export function useWaChats(handle: string | undefined, includeBroadcasts: boolean) {
  return useQuery({
    queryKey: ["wa", "chats", handle, includeBroadcasts],
    queryFn: () =>
      call<Paged<ConversationSummary>>("wa_chats", {
        handle,
        page: { offset: 0, limit: 2000 },
        includeBroadcasts,
      }),
    enabled: Boolean(handle),
  });
}

/** One chat, newest first, loading older pages on demand. */
export function useWaMessages(handle: string | undefined, chatId: string | null) {
  return useInfiniteQuery({
    queryKey: ["wa", "messages", handle, chatId],
    enabled: Boolean(handle) && Boolean(chatId),
    initialPageParam: null as MessageCursor | null,
    queryFn: ({ pageParam }) =>
      call<MessagePage>("wa_messages", { handle, chatId, cursor: pageParam }),
    getNextPageParam: (last) => last.older ?? undefined,
  });
}
