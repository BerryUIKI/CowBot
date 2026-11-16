# ADR 0050: Desktop interface internationalisation

- Status: Accepted
- Date: 2026-11-16
- Discussion: Requested with the desktop language picker redesign.
- Supersedes: None. Extends the desktop shell in
  [ADR-0024](0024-agent-direct-conversations.md) and the Settings surface in
  [ADR-0030](0030-conversation-sessions-and-context.md).

## Context

Every string in the desktop frontend was written directly in JSX, in English.
The product now needs an interface the user can read in their own language and
that the assistant can work with: English, Simplified Chinese, and Traditional
Chinese. The request also asked for the language switcher to follow Claude's
language picker and for the bottom-left user area to follow Claude's account
menu.

Localisation is not just translation. The app must keep key parity across
catalogues, remember the preference on the device, format dates with the chosen
language, and leave a path for more languages without rewriting components.
The Rust backend is a separate concern: it produces diagnostic messages, agent
profiles, audit reasons, and stored data, all of which may be read by CLI and
server clients that have no locale preference.

## Decision

### 1. The frontend owns presentation copy

`desktop/src/i18n/en.ts` is the source catalogue. It is a nested object of
strings, and `Catalog = typeof en` gives every other catalogue a structural
type. `zh-CN.ts` and `zh-TW.ts` are declared as `Catalog`, so the TypeScript
build fails when a key is missing, misspelled, or extra. There is no runtime
fallback to a half-translated catalogue; English remains the final fallback only
for defensive lookups.

Keys are grouped by feature and named after the control, not the English
wording (`sidebar.searchPlaceholder`, `settings.approvals.rulesTitle`). A
wording change never renames a key. Components call `useT()` or `useI18n()`;
plain modules that produce labels (task status, approval modes, background
activity, slash command summaries) call `t` from `i18n/catalog`, which reads the
locale set by the provider during render. This split keeps `i18n/catalog.ts`
free of React and Tauri imports.

### 2. The shipped locales are a registry, not a switch statement

`localeDefinitions` maps each locale to its native name, English name, short
label, BCP 47 tag, and catalogue. English, Simplified Chinese, and Traditional
Chinese are shipped. Adding a locale is a new file plus one registry entry; the
type checker proves the catalogue is complete. `system` resolves the operating
system language, treating `Hant`, `TW`, `HK`, and `MO` tags as Traditional
Chinese and every other Chinese tag as Simplified Chinese.

Traditional Chinese was seeded from the Simplified catalogue with OpenCC
(`cn` → `twp`) and then reviewed against Taiwan software terminology (for
example 代理, 設定, 檔案, 儲存, 對話, 核准, 稽核, 範本, 計畫, 預設,
回覆). It is stored as a normal static catalogue, not converted at runtime, so
the bundle has no conversion dependency and translators can edit it directly.

### 3. The preference is device-local

`AppConfig` gains `ui_locale: Option<String>`, and the Tauri commands
`get_ui_locale` and `set_ui_locale` read and write it. The frontend normalises
the value to `system`, `en`, `zh-CN`, or `zh-TW`; an unknown value follows the
system. The provider also caches the preference in localStorage so the first
paint after a restart already uses the right language, then reconciles with the
configuration file. The preference is per device and never enters a workspace
package, the server, or an agent prompt.

### 4. The picker follows Claude's language dialog

The language picker is a modal grid of shipped languages, each row showing the
language in its own script and its English name underneath, with a check on the
active one. It is opened from **Settings → General** and from the account menu.
The settings row shows the current language and opens the same dialog, so there
is one picker to maintain.

### 5. The bottom-left account row follows Claude's account menu

The footer shows `User Name · Workspace Name` with the user's avatar and a
chevron, plus the existing Settings gear. The row opens a menu with the name and
workspace at the top, then Settings, Language, Model configuration, Workspace
settings, View changelog, and Learn more. Settings and workspace entries open
the existing Settings sections; changelog and learn-more open the GitHub
releases page and repository. There is no sign-out because the local workspace
has no account session to end.

### 6. Formatting follows the interface language

`format.ts` caches `Intl.DateTimeFormat` instances per active locale, so times,
dates, and day separators use the chosen language rather than the WebView's
default. Token counts stay compact and language-neutral.

### 7. The backend boundary is explicit

The Rust service and store keep producing English diagnostics, agent profiles,
template content, audit actions, and persisted reasons. The desktop translates
the copy it owns: labels, menus, buttons, empty states, confirmations, status
names, command summaries, and memory kinds. A service error is displayed as
returned because it is diagnostic evidence, not presentation copy. Agent replies
follow the language of the user's messages; they are not forced to the device's
interface language, because one workspace may be opened from devices with
different languages.

## Alternatives considered

- **i18next or react-intl.** Mature plural and ICU handling, but a runtime
  dependency, namespace loading, and string-key conventions that TypeScript
  cannot prove complete. The app's copy needs no plurals library beyond a few
  explicit `one`/`other` keys, so typed static catalogues buy more safety with
  less machinery.
- **English strings as keys.** Fast to extract, but renames and translations
  collide, interpolated copy becomes unreadable, and the source catalogue cannot
  be reviewed as a whole.
- **Runtime OpenCC conversion for Traditional Chinese.** Avoids a second file
  but ships a conversion table, prevents per-string review, and makes the
  catalogue the output of an algorithm rather than a translatable artifact.
- **Localise the Rust backend and CLI.** Would add locale plumbing to shared
  crates and server responses that may be read without a UI. Deferred until
  error codes are exposed across the API; see the consequences.
- **Pin agent reply language to the UI locale.** Wrong for shared workspaces and
  remote clients, and unnecessary: a model already answers in the language the
  user writes.

## Consequences

- A new UI string requires one English key and three translations; without all
  four the build fails.
- All catalogues are bundled eagerly. At this size the cost is small, and it
  keeps the first paint synchronous; lazy loading can be added behind the same
  `t` API later.
- The backend's English messages still appear inside translated chrome. When
  the server exposes stable error codes, the desktop can map them to localized
  copy; the audit and prompt-snapshot views will continue to show stored
  evidence as recorded.
- Right-to-left layout, locale-aware sorting, and plural categories beyond
  `one`/`other` are not implemented. Adding an RTL language will need a
  direction pass in the shell CSS.
- The slash command catalogue's names, usage, and argument placeholders remain
  protocol data from `cowbot-core`; only summaries and desktop-side hints are
  localized. Localizing usage would change the command syntax shared with the
  CLI and server.
