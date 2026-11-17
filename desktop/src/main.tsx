import { StrictMode } from "react";
import { createRoot } from "react-dom/client";

import App from "./App";
import StartupGate from "./StartupGate";
import { I18nProvider } from "./i18n";
import { UiPreferencesProvider } from "./uiPreferences";
import "./styles.css";

createRoot(document.getElementById("root") as HTMLElement).render(
  <StrictMode>
    <I18nProvider>
      <UiPreferencesProvider>
        <StartupGate>
          <App />
        </StartupGate>
      </UiPreferencesProvider>
    </I18nProvider>
  </StrictMode>,
);
