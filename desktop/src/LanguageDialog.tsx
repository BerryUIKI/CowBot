import { useState } from "react";

import { errorMessage } from "./errors";
import { useI18n, localeDefinitions, supportedLocales } from "./i18n";
import type { Locale } from "./i18n";
import Modal from "./Modal";

/**
 * Claude-style language picker: a grid of every shipped language, each shown
 * in its own script with its English name underneath. The resolved locale is
 * checked; choosing one stores it as an explicit preference and applies it
 * immediately.
 */
export default function LanguageDialog({ onClose }: { onClose: () => void }) {
  const { t, locale, setPreference } = useI18n();
  const [error, setError] = useState<string | null>(null);

  async function choose(next: Locale) {
    setError(null);
    try {
      await setPreference(next);
    } catch (err) {
      setError(errorMessage(err));
    }
  }

  return (
    <Modal
      title={t("language.chooseTitle")}
      onClose={onClose}
      className="language-modal"
    >
      <div
        className="language-grid"
        role="listbox"
        aria-label={t("language.chooseTitle")}
      >
        {supportedLocales.map((value) => {
          const definition = localeDefinitions[value];
          const selected = locale === value;
          return (
            <button
              key={value}
              type="button"
              role="option"
              aria-selected={selected}
              className={`language-option${selected ? " is-selected" : ""}`}
              onClick={() => void choose(value)}
            >
              <span className="language-option-copy">
                <span className="language-option-native">
                  {definition.nativeName}
                </span>
                <span className="language-option-english">
                  {definition.englishName}
                </span>
              </span>
              {selected ? (
                <svg
                  className="language-option-check"
                  viewBox="0 0 24 24"
                  fill="none"
                  stroke="currentColor"
                  strokeWidth="2"
                  aria-hidden="true"
                >
                  <path d="m5 12 5 5L20 7" />
                </svg>
              ) : null}
            </button>
          );
        })}
      </div>
      {error ? (
        <p className="dialog-error" role="alert">
          {t("language.saveError", { error })}
        </p>
      ) : null}
    </Modal>
  );
}
