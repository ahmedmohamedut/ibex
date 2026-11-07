/**
 * Placeholder shell. The real layout (top bar, sidebar, list/detail panes) and
 * the Welcome screen land in milestone M1; see docs/UI_DESIGN.md.
 */
export default function App() {
  return (
    <main
      style={{
        display: "grid",
        placeContent: "center",
        height: "100%",
        gap: "0.5rem",
        textAlign: "center",
      }}
    >
      <h1 style={{ margin: 0, fontSize: "1.5rem" }}>ibex</h1>
      <p style={{ margin: 0, color: "var(--text-muted)" }}>
        iOS Backup Explorer &middot; the UI arrives in milestone M1
      </p>
    </main>
  );
}
