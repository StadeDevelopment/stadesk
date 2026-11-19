import { useEffect, useState } from "react";
import { api, type Overview } from "./api";
import { LanguageContext, resolveLanguage } from "./i18n";
import { MainWindow } from "./MainWindow";
import { ViewerWindow } from "./ViewerWindow";

function viewerSession(): number | null {
  const match = /^#\/viewer\/(\d+)$/.exec(window.location.hash);
  return match ? Number(match[1]) : null;
}

export function App() {
  const [overview, setOverview] = useState<Overview | null>(null);
  const session = viewerSession();

  useEffect(() => {
    api.overview().then(setOverview).catch(() => setOverview(null));
  }, []);

  const language = resolveLanguage(overview?.settings.language);
  useEffect(() => {
    document.documentElement.lang = language;
  }, [language]);

  return (
    <LanguageContext.Provider value={language}>
      {session !== null ? (
        <ViewerWindow sessionId={session} />
      ) : overview ? (
        <MainWindow overview={overview} onOverview={setOverview} />
      ) : null}
    </LanguageContext.Provider>
  );
}
