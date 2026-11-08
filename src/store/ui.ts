import { create } from "zustand";
import type { BackupSummary } from "../bindings/BackupSummary";

/** Which sidebar section is showing. */
export type Section =
  | "overview"
  | "messages"
  | "whatsapp"
  | "photos"
  | "voice"
  | "documents"
  | "raw"
  | "exports";

export type Theme = "system" | "light" | "dark";

const THEME_KEY = "ibex.theme";

/** Read the saved theme. Storage can throw in a locked-down webview. */
function storedTheme(): Theme {
  try {
    const value = localStorage.getItem(THEME_KEY);
    if (value === "light" || value === "dark" || value === "system") return value;
  } catch {
    // Ignore: the default is fine.
  }
  return "system";
}

/** Apply a theme by setting (or clearing) `data-theme`, which tokens.css keys off. */
export function applyTheme(theme: Theme): void {
  const root = document.documentElement;
  if (theme === "system") root.removeAttribute("data-theme");
  else root.setAttribute("data-theme", theme);
}

/** Where a search result wants the UI to jump to. */
export interface PendingJump {
  source: "sms" | "whatsapp";
  conversationId: string;
  messageId: string;
}

interface UiState {
  /** The open backup, or null on the Welcome screen. */
  backup: BackupSummary | null;
  section: Section;
  /** Raw browser: the selected domain. */
  domain: string | null;
  /** Raw browser: directory within the domain. */
  dir: string;
  /** Raw browser: the selected file's id. */
  fileId: string | null;
  theme: Theme;
  /** Whether the Ctrl+K search palette is open. */
  searchOpen: boolean;
  /** Set by the search palette when a result is opened; consumed (and
   * cleared) by the Messages/WhatsApp view it targets. */
  pendingJump: PendingJump | null;

  openedBackup: (backup: BackupSummary) => void;
  closedBackup: () => void;
  setSection: (section: Section) => void;
  selectDomain: (domain: string | null) => void;
  setDir: (dir: string) => void;
  selectFile: (fileId: string | null) => void;
  setTheme: (theme: Theme) => void;
  openSearch: () => void;
  closeSearch: () => void;
  jumpTo: (jump: PendingJump) => void;
  clearPendingJump: () => void;
}

export const useUi = create<UiState>((set) => ({
  backup: null,
  section: "overview",
  domain: null,
  dir: "",
  fileId: null,
  theme: storedTheme(),
  searchOpen: false,
  pendingJump: null,

  openedBackup: (backup) =>
    set({ backup, section: "overview", domain: null, dir: "", fileId: null }),
  closedBackup: () =>
    set({ backup: null, section: "overview", domain: null, dir: "", fileId: null }),
  setSection: (section) => set({ section }),
  openSearch: () => set({ searchOpen: true }),
  closeSearch: () => set({ searchOpen: false }),
  jumpTo: (jump) =>
    set({
      pendingJump: jump,
      searchOpen: false,
      section: jump.source === "sms" ? "messages" : "whatsapp",
    }),
  clearPendingJump: () => set({ pendingJump: null }),
  // Changing domain resets the path and selection, or the detail pane would
  // show a file from the previous domain.
  selectDomain: (domain) => set({ domain, dir: "", fileId: null }),
  setDir: (dir) => set({ dir, fileId: null }),
  selectFile: (fileId) => set({ fileId }),
  setTheme: (theme) => {
    try {
      localStorage.setItem(THEME_KEY, theme);
    } catch {
      // Not fatal: the theme simply won't persist.
    }
    applyTheme(theme);
    set({ theme });
  },
}));
