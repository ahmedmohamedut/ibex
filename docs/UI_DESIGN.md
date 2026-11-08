# UI design

## Goals

- Feel like a native file/messaging app, not a database viewer.
- Fast on large backups (100k+ messages, 50k+ photos): everything virtualized, everything paginated.
- Export is always one or two clicks away from whatever you're looking at.
- Missing data is visible and explained, not hidden.

## Layout

```
┌──────────────────────────────────────────────────────────────────────────────┐
│ ◀ iPhone 15 · iOS 18.1 · 2026-11-01 · 🔒            [ Search… ⌘K ]  [Export] │  top bar
├────────────┬──────────────────────────┬──────────────────────────────────────┤
│ Overview   │  list pane               │  detail pane                         │
│ Messages 4k│  (conversations, files,  │  (chat thread, photo viewer,         │
│ WhatsApp   │   recordings…)           │   file preview…)                     │
│ Photos     │                          │                                      │
│ Voice Memos│                          │                                      │
│ Files      │                          │                                      │
│ Raw        │                          │                                      │
│ ─────────  │                          │                                      │
│ Exports    │                          │                                      │
└────────────┴──────────────────────────┴──────────────────────────────────────┘
                                                   job toasts bottom-right ▸
```

- **Sidebar** (≈200px): sections with counts. Sections whose source is absent are greyed with a tooltip explaining why ("WhatsApp not installed or not backed up").
- **List pane** (≈320px, resizable): virtualized list for the section.
- **Detail pane**: the selected item.
- Photos uses list + detail merged into a full-width grid with a lightbox.

## Screens

### Welcome (no backup open)
- Big "Open backup folder…" button (Tauri dialog, directory mode).
- Recent backups list: device name, iOS version, backup date, size, encrypted badge.
- Drag-and-drop a folder onto the window opens it.
- Validation errors in plain words: "This folder has no Manifest.db. Pick the folder named after the device's UDID."

### Password prompt (encrypted backup)
Modal with password field, "Show" toggle, explanation that decryption happens locally and decrypted data is cached under `~/.local/share/ibex` until cleared. Wrong password → inline error, field kept.

### Overview
Cards: device info (name, model, iOS, serial last 4, backup date, encrypted), counts per source (conversations/messages, chats, photos/videos and how many are cloud-only, recordings, documents), total backup size, cache size with "Clear cache".

### Messages (SMS/iMessage) and WhatsApp
Same components, different source.

- **List pane:** conversation rows: avatar initials, name (contact name, else number/email, else group name), last message preview, date, message count, service icon. Filter box on top. Sort by last activity.
- **Detail pane:** header (name, participants for groups, message count, date range, Export button). Thread:
  - Bubbles: right/accent for `from_me`, left/neutral for others; sender name above bubble in groups.
  - Day separators ("Tuesday, 3 March 2026").
  - Timestamps on hover; edited/deleted markers.
  - Reactions as small chips under the bubble.
  - Replies show a quoted snippet of the parent.
  - Attachments inline: images as thumbnails (click → lightbox), video poster with play, voice notes as a compact `AudioPlayer` (waveform optional, duration required), documents as file cards (icon, name, size, "Save as…").
  - Missing attachment → grey card "Not in backup".
  - System messages centered, small, muted.
- **Jump to date** control in the header. Opening a thread starts at the newest message; scrolling up loads older pages (reverse infinite scroll, keep scroll position stable).

### Photos
- Toolbar: filters (All / Photos / Videos / Favorites / Screenshots later), year/month jump, "Show cloud-only" toggle, selection mode.
- Grid: virtualized, square thumbnails (≈160px), grouped by month headers. Video badge with duration, Live badge, cloud-only placeholder with cloud icon.
- Lightbox: full image (HEIC served as cached JPEG), metadata side panel (date, dimensions, file name, original name, location coordinates, album membership), arrow keys to navigate, Export.
- Multi-select with shift/ctrl click and rubber-band → "Export N items".

### Voice Memos
Table: title, date, duration, size. Row click → player in detail pane with title, date, scrubber. Export selected / all with names `<date> <title>.m4a`.

### Files
- Sources in the list pane: "On My iPhone" (Files app) + one entry per app with documents (app name + bundle id).
- Detail pane: breadcrumb + file table (name, kind, size, modified). Double-click a folder to enter. Preview pane for: images, PDF (WebKit's built-in viewer via `ibex://`), text/JSON/plist (pretty-printed), audio/video; else hex.

### Raw
Two-level browser: domains (with file count and size) → tree of the domain's paths. File preview identical to Files. Extra info: fileID, physical path, flags, protection class, raw metadata. "Copy fileID", "Save as…", "Open containing physical folder".

### Contacts
- List pane: avatar initials, name (or organization when there's no personal name), filter box, "Export all" (vCard). Ordered by last name then first.
- Detail pane: name, organization, nickname, every phone number and email on file, "Export vCard" for just this contact.

### Call History
Single full-width table (no detail pane — there's nothing more to show than the row already has): contact name or raw number, direction icon (outgoing/incoming/missed, missed shown in the danger color), date, duration, service (Cellular or FaceTime, with a video icon for FaceTime). Names resolve through the same address-book matching Messages/WhatsApp use.

### Search (⌘K / Ctrl+K)
Command palette overlay. First use prompts to build the index (job with progress). Results grouped by service → conversation, with highlighted snippets; Enter opens the thread scrolled to that message (highlight it briefly).

### Export dialog
Opened from a conversation, a selection, a section, or the top bar.

1. **Scope:** this conversation / selected items / whole section / everything (checkbox tree of sections).
2. **Format:** HTML (default for chats), CSV, JSON, Original files (default for photos/voice/files). HTML/CSV/JSON options: include attachments (copy media), date range.
3. **Destination:** folder picker; shows estimated size.
4. Start → becomes a job toast with progress, cancel, and "Open folder" when finished. Report summary: exported, skipped, missing.

### Exports (sidebar)
History of exports in this session: scope, format, destination, status, report.

## Interaction rules

- Every long-running action is a job with progress and cancel. The UI never blocks.
- Keyboard: Ctrl+K search, Ctrl+E export, Ctrl+O open backup, arrows in lists and lightbox, Esc closes overlays.
- Empty states always say why and what to do.
- Errors: toast with a short message and "Details" (error code + technical message, no personal data).

## Visual style

- System font stack (`system-ui, -apple-system, "Segoe UI", Cantarell, "Noto Sans", sans-serif`); monospace for fileIDs and hex.
- Density: compact, 13–14px base, 8px spacing grid.
- Light and dark themes following `prefers-color-scheme`, with a manual override.
- Icons: bundle `lucide-react` (no CDN).

### Theme tokens (`src/styles/tokens.css`)

```css
:root {
  --bg: #ffffff;
  --bg-elev: #f6f6f7;
  --bg-sidebar: #efeff1;
  --border: #e2e2e6;
  --text: #1c1c1e;
  --text-muted: #6b6b73;
  --accent: #0a7cff;
  --accent-text: #ffffff;
  --bubble-me: #0a7cff;
  --bubble-me-text: #ffffff;
  --bubble-them: #e9e9eb;
  --bubble-them-text: #1c1c1e;
  --bubble-wa-me: #d9fdd3;
  --bubble-wa-me-text: #111b21;
  --danger: #d93025;
  --warning: #b26a00;
  --radius: 8px;
  --radius-bubble: 18px;
}
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) {
    --bg: #1c1c1e;
    --bg-elev: #2c2c2e;
    --bg-sidebar: #232325;
    --border: #3a3a3c;
    --text: #f2f2f7;
    --text-muted: #9a9aa1;
    --accent: #3d95ff;
    --bubble-them: #2c2c2e;
    --bubble-them-text: #f2f2f7;
    --bubble-wa-me: #005c4b;
    --bubble-wa-me-text: #e9edef;
  }
}
:root[data-theme="dark"] { /* same values as the dark block above */ }
```

## HTML export look

Self-contained `index.html` per conversation that mirrors the in-app thread (same bubble tokens, light theme by default, dark via `prefers-color-scheme`), a header with participants and date range, `media/` folder with relative links, no JavaScript required to read. A top-level `index.html` lists all exported conversations.
