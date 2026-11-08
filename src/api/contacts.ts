import { useQuery } from "@tanstack/react-query";
import { call } from "./client";
import type { Contact } from "../bindings/Contact";
import type { Paged } from "../bindings/Paged";

/** Every contact, ordered by last name then first. */
export function useContacts(handle: string | undefined) {
  return useQuery({
    queryKey: ["contacts", handle],
    queryFn: () =>
      call<Paged<Contact>>("contacts_list", {
        handle,
        page: { offset: 0, limit: 2000 },
      }),
    enabled: Boolean(handle),
  });
}
