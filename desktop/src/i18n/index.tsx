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

import { getUiLocale, setUiLocale } from "../api";
import {
  localeDefinitions,
  normalizeLocalePreference,
  resolveLocale,
  setActiveLocale,
  translate,
  type Locale,
  type LocalePreference,
  type TranslationKey,
  type TranslationVars,
} from "./catalog";

export * from "./catalog";

/**
 * React interface for the locale registry in `./catalog`.
 *
 * The preference lives in the desktop configuration file and is cached in
 * localStorage so the first paint after a restart already uses the right
 * language. Re-exporting the catalogue API keeps component imports short.
 */

const PREFERENCE_CACHE_KEY = "cowbot.uiLocale";

export interface I18nContextValue {
  locale: Locale;
  preference: LocalePreference;
  intlLocale: string;
  t: (key: TranslationKey, vars?: TranslationVars) => string;
  setPreference: (preference: LocalePreference) => Promise<void>;
}

const I18nContext = createContext<I18nContextValue | null>(null);

function readCachedPreference(): LocalePreference {
  try {
    const cached = window.localStorage.getItem(PREFERENCE_CACHE_KEY);
    return cached === null ? "system" : normalizeLocalePreference(cached);
  } catch {
    return "system";
  }
}

function writeCachedPreference(preference: LocalePreference): void {
  try {
    window.localStorage.setItem(PREFERENCE_CACHE_KEY, preference);
  } catch {
    // A read-only WebView storage must not stop the language switch.
  }
}

export function I18nProvider({ children }: { children: ReactNode }) {
  const [preference, setPreferenceState] =
    useState<LocalePreference>(readCachedPreference);
  const changedLocally = useRef(false);

  const locale = resolveLocale(preference);
  // Set before children render so module-level helpers use the new language
  // in the same pass as the provider's consumers.
  setActiveLocale(locale);
  const intlLocale = localeDefinitions[locale].intl;

  const setPreference = useCallback(
    async (next: LocalePreference) => {
      changedLocally.current = true;
      setPreferenceState(next);
      writeCachedPreference(next);
      await setUiLocale(next);
    },
    [],
  );

  useEffect(() => {
    let cancelled = false;
    getUiLocale()
      .then((stored) => {
        if (cancelled || changedLocally.current) return;
        const next = normalizeLocalePreference(stored);
        setPreferenceState(next);
        writeCachedPreference(next);
      })
      .catch(() => {
        // The cached preference keeps working when the backend cannot answer.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  useEffect(() => {
    document.documentElement.lang = intlLocale;
  }, [intlLocale]);

  const value = useMemo<I18nContextValue>(
    () => ({
      locale,
      preference,
      intlLocale,
      t: (key, vars) => translate(locale, key, vars),
      setPreference,
    }),
    [intlLocale, locale, preference, setPreference],
  );

  return <I18nContext.Provider value={value}>{children}</I18nContext.Provider>;
}

export function useI18n(): I18nContextValue {
  const value = useContext(I18nContext);
  if (!value) throw new Error("useI18n must be used inside I18nProvider");
  return value;
}

/** Convenience hook for components that only need `t`. */
export function useT(): I18nContextValue["t"] {
  return useI18n().t;
}
