import { en, type Catalog } from "./en";
import { zhCN } from "./zh-CN";
import { zhTW } from "./zh-TW";

/**
 * Interface locale registry and translation core.
 *
 * This module has no React or Tauri dependency so plain modules (status
 * labels, date formatting, command planning) can translate without importing
 * the provider. `i18n/index.tsx` adds the React context and persistence.
 */

export type Locale = "en" | "zh-CN" | "zh-TW";
export type LocalePreference = "system" | Locale;

export interface LocaleDefinition {
  /** Language name shown in the picker, written in that language. */
  nativeName: string;
  /** English name shown under the native name in the language picker. */
  englishName: string;
  /** Short label for menus and the settings row. */
  shortName: string;
  /** BCP 47 tag passed to `Intl`. */
  intl: string;
  catalog: Catalog;
}

export const localeDefinitions: Record<Locale, LocaleDefinition> = {
  en: {
    nativeName: "English (United States)",
    englishName: "English (United States)",
    shortName: "English",
    intl: "en",
    catalog: en,
  },
  "zh-CN": {
    nativeName: "简体中文（中国）",
    englishName: "Chinese (Simplified, China)",
    shortName: "简体中文",
    intl: "zh-CN",
    catalog: zhCN,
  },
  "zh-TW": {
    nativeName: "繁體中文（台灣）",
    englishName: "Chinese (Traditional, Taiwan)",
    shortName: "繁體中文",
    intl: "zh-TW",
    catalog: zhTW,
  },
};

export const supportedLocales = Object.keys(localeDefinitions) as Locale[];

/** Dotted paths of every string leaf in the catalogue. */
type StringLeafPaths<T> = {
  [K in keyof T & string]: T[K] extends string
    ? K
    : `${K}.${StringLeafPaths<T[K]>}`;
}[keyof T & string];

export type TranslationKey = StringLeafPaths<Catalog>;

export type TranslationVars = Record<string, string | number>;

function isLocale(value: unknown): value is Locale {
  return (
    typeof value === "string" && Object.hasOwn(localeDefinitions, value)
  );
}

/** Validate a stored or received preference, falling back to `system`. */
export function normalizeLocalePreference(value: unknown): LocalePreference {
  if (value === "system") return "system";
  return isLocale(value) ? value : "system";
}

/** The locale to use when the preference follows the operating system. */
export function systemLocale(): Locale {
  if (typeof navigator === "undefined") return "en";
  const languages =
    navigator.languages && navigator.languages.length > 0
      ? navigator.languages
      : [navigator.language];
  const chinese = languages.find((language) =>
    language.toLowerCase().startsWith("zh"),
  );
  if (!chinese) return "en";
  const tag = chinese.toLowerCase();
  return tag.includes("hant") ||
    tag.includes("-tw") ||
    tag.includes("-hk") ||
    tag.includes("-mo")
    ? "zh-TW"
    : "zh-CN";
}

export function resolveLocale(preference: LocalePreference): Locale {
  return preference === "system" ? systemLocale() : preference;
}

let activeLocale: Locale = resolveLocale("system");

/** Switch the locale used by `t` and `translate` for non-component helpers. */
export function setActiveLocale(locale: Locale): void {
  activeLocale = locale;
}

/** The locale the UI is currently rendering in. */
export function currentLocale(): Locale {
  return activeLocale;
}

/** BCP 47 tag for `Intl` formatting calls. */
export function currentIntlLocale(): string {
  return localeDefinitions[activeLocale].intl;
}

function lookup(catalog: Catalog, key: string): string | undefined {
  let node: unknown = catalog;
  for (const part of key.split(".")) {
    if (typeof node !== "object" || node === null) return undefined;
    node = (node as Record<string, unknown>)[part];
  }
  return typeof node === "string" ? node : undefined;
}

function interpolate(template: string, vars?: TranslationVars): string {
  if (!vars) return template;
  return template.replace(/\{(\w+)\}/g, (match, name: string) => {
    const value = vars[name];
    return value === undefined ? match : String(value);
  });
}

/**
 * Translate a key in a given locale.
 *
 * The English catalogue is the final fallback so a partially reviewed
 * translation never renders a raw key.
 */
export function translate(
  locale: Locale,
  key: TranslationKey,
  vars?: TranslationVars,
): string {
  const value =
    lookup(localeDefinitions[locale].catalog, key) ?? lookup(en, key) ?? key;
  return interpolate(value, vars);
}

/** Translate using the locale the UI is currently rendering in. */
export function t(key: TranslationKey, vars?: TranslationVars): string {
  return translate(activeLocale, key, vars);
}
