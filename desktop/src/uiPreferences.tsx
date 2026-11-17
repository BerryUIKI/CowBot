import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

import { getShowAgentBotTags, setShowAgentBotTags as saveAgentBotTags } from "./api";

/**
 * Device-local interface preferences that are not language.
 *
 * The bot tag is a presentation switch: agent names stay workspace data, and
 * the choice lives beside the locale in `config.json`. The value is cached in
 * localStorage so the first paint after a restart already knows whether to tag
 * agent names, then reconciled with the configuration file.
 */

const CACHE_KEY = "cowbot.showAgentBotTags";

export interface UiPreferencesValue {
  showAgentBotTags: boolean;
  setShowAgentBotTags: (enabled: boolean) => Promise<void>;
}

const UiPreferencesContext = createContext<UiPreferencesValue | null>(null);

function readCachedFlag(): boolean {
  try {
    return window.localStorage.getItem(CACHE_KEY) === "1";
  } catch {
    return false;
  }
}

function writeCachedFlag(enabled: boolean): void {
  try {
    window.localStorage.setItem(CACHE_KEY, enabled ? "1" : "0");
  } catch {
    // A read-only WebView storage must not stop the switch from working.
  }
}

export function UiPreferencesProvider({ children }: { children: ReactNode }) {
  const [showAgentBotTags, setFlag] = useState(readCachedFlag);
  const changedLocally = useRef(false);

  const setShowAgentBotTags = useCallback(async (enabled: boolean) => {
    changedLocally.current = true;
    setFlag(enabled);
    writeCachedFlag(enabled);
    await saveAgentBotTags(enabled);
  }, []);

  useEffect(() => {
    let cancelled = false;
    getShowAgentBotTags()
      .then((stored) => {
        if (cancelled || changedLocally.current) return;
        setFlag(stored);
        writeCachedFlag(stored);
      })
      .catch(() => {
        // The cached flag keeps working when the backend cannot answer.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  const value = useMemo<UiPreferencesValue>(
    () => ({ showAgentBotTags, setShowAgentBotTags }),
    [showAgentBotTags, setShowAgentBotTags],
  );

  return (
    <UiPreferencesContext.Provider value={value}>
      {children}
    </UiPreferencesContext.Provider>
  );
}

export function useUiPreferences(): UiPreferencesValue {
  const value = useContext(UiPreferencesContext);
  if (!value) {
    throw new Error("useUiPreferences must be used inside UiPreferencesProvider");
  }
  return value;
}
