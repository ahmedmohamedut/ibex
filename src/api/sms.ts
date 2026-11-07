import { useInfiniteQuery, useQuery } from "@tanstack/react-query";
import { call } from "./client";
import type { ConversationSummary } from "../bindings/ConversationSummary";
import type { MessageCursor } from "../bindings/MessageCursor";
import type { MessagePage } from "../bindings/MessagePage";
import type { Paged } from "../bindings/Paged";

/** Conversations, most recently active first. */
export function useSmsConversations(handle: string | undefined) {
  return useQuery({
    queryKey: ["sms", "conversations", handle],
    queryFn: () =>
      call<Paged<ConversationSummary>>("sms_conversations", {
        handle,
        page: { offset: 0, limit: 500 },
      }),
    enabled: Boolean(handle),
  });
}

/**
 * One conversation, newest first, loading older pages on demand.
 *
 * The cursor is a keyset, not an offset, so paging a long thread stays exact
 * and cheap however deep it goes.
 */
export function useSmsMessages(
  handle: string | undefined,
  conversationId: string | null,
) {
  return useInfiniteQuery({
    queryKey: ["sms", "messages", handle, conversationId],
    enabled: Boolean(handle) && Boolean(conversationId),
    initialPageParam: null as MessageCursor | null,
    queryFn: ({ pageParam }) =>
      call<MessagePage>("sms_messages", {
        handle,
        conversationId,
        cursor: pageParam,
      }),
    getNextPageParam: (last) => last.older ?? undefined,
  });
}
