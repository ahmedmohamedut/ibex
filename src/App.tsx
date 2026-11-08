import { QueryClient, QueryClientProvider } from "@tanstack/react-query";
import { useEffect } from "react";
import { Sidebar } from "./components/Sidebar";
import { Toasts } from "./components/Toasts";
import { TopBar } from "./components/TopBar";
import { applyTheme, useUi } from "./store/ui";
import { Exports } from "./views/Exports";
import { JobToasts } from "./components/JobToasts";
import { Messages } from "./views/Messages";
import { Overview } from "./views/Overview";
import { Photos } from "./views/Photos";
import { Raw } from "./views/Raw";
import { WhatsApp } from "./views/WhatsApp";
import { Welcome } from "./views/Welcome";

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // Backup contents cannot change under us — the folder is read-only — so
      // there is nothing to refetch on focus.
      refetchOnWindowFocus: false,
      staleTime: 60_000,
      retry: false,
    },
  },
});

function Shell() {
  const backup = useUi((s) => s.backup);
  const section = useUi((s) => s.section);
  const theme = useUi((s) => s.theme);

  // Apply the stored theme on mount and whenever it changes.
  useEffect(() => {
    applyTheme(theme);
  }, [theme]);

  return (
    <div className="app">
      <TopBar />
      {backup ? (
        <div className="body">
          <Sidebar />
          {section === "raw" ? (
            <Raw />
          ) : section === "messages" ? (
            <Messages />
          ) : section === "whatsapp" ? (
            <WhatsApp />
          ) : section === "photos" ? (
            <Photos />
          ) : section === "exports" ? (
            <Exports />
          ) : (
            <Overview />
          )}
        </div>
      ) : (
        <Welcome />
      )}
      <JobToasts />
      <Toasts />
    </div>
  );
}

export default function App() {
  return (
    <QueryClientProvider client={queryClient}>
      <Shell />
    </QueryClientProvider>
  );
}
