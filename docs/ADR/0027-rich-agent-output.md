# ADR 0027: Rich Agent Output

- **Status**: Accepted
- **Date**: 2026-11-09
- **Related**: `docs/ADR/0028-main-assistant-and-workspace-actions.md`, `docs/ARCHITECTURE.md` §4, `docs/USER_GUIDE.md`

## Context

Agents answer in Markdown. In 0.1.5 the desktop rendered every message as plain text, so `**bold**`, numbered lists, tables, and code blocks appeared literally. Agents also had no way to present an interactive result: a form, a small calculator, or a chart had to be described in prose, and raw HTML was displayed as text.

The product wants the work page to render agent output the way ChatGPT renders Markdown and HTML artifacts. That means two different renderers with different trust levels:

- Inline Markdown and formatting HTML inside a message.
- A complete HTML document that may contain CSS and JavaScript and that the human can interact with.

The desktop webview runs with `csp: null` in `tauri.conf.json`, so rendering model-generated HTML without a boundary would let the model execute code in the app's origin, reach the Tauri API, and read the workspace.

## Decision

### 1. Markdown and inline HTML are sanitized before rendering

`desktop/src/RichText.tsx` renders Markdown with `react-markdown`, `remark-gfm` (tables, task lists, autolinks), `rehype-raw` (parse inline HTML), and `rehype-sanitize` with an extended GitHub schema:

- Commonmark/GFM formatting, tables, code, blockquotes, and images are allowed.
- `script`, `style`, `iframe`, `object`, embedded forms, inline event handlers, and `style` attributes are removed.
- Link protocols are limited to `http`, `https`, and `mailto`; image sources are limited to `data:` so a model cannot make the app fetch a remote resource without the user acting.
- Links are intercepted and opened in the system browser through the Tauri opener plugin; the webview never navigates away from the app.

The same component renders chat bubbles (user and agent) and task deliverables. System notices remain plain text because they are app-generated. Sidebar previews use a plain-text stripper (`plainTextPreview`) so Markdown or HTML never leaks into the contact list.

### 2. HTML artifacts run in a sandboxed iframe

A fenced code block whose language is `html` or `htm` is not rendered as code. `desktop/src/HtmlArtifact.tsx` wraps it in a preview card with **Preview/Code** and **Expand**, and renders the preview in an iframe:

- `sandbox="allow-scripts allow-forms"` without `allow-same-origin`, `allow-top-navigation`, or `allow-popups`. The document gets an opaque origin, cannot read the parent DOM, storage, cookies, or the Tauri API, and cannot navigate the app.
- The srcdoc starts with a CSP: `default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; img-src data:; font-src data:; connect-src 'none'; form-action 'none'; base-uri 'none'; frame-src 'none'; object-src 'none'`. Inline CSS and JavaScript work; network requests, external images/fonts, form submission, and nested frames do not.
- `<base>` and `<meta http-equiv="refresh">` are stripped before wrapping.
- The card is a normal message block: fixed preview height with internal scrolling, a code view of the exact source, and a modal for a larger view.

Full documents keep their own `<head>`; a fragment is wrapped in a minimal document with a readable default style.

### 3. Interaction stays inside the artifact for now

Scripts can update the artifact's own UI (tabs, forms, charts, small tools), but there is no artifact-to-chat message bridge: a button inside the iframe cannot send a message to the agent or the app. The human reads the result and replies in the composer. A future bridge would need a per-artifact token, a validated `postMessage` protocol, and a clear rule for what reaches the model; it is deliberately not part of this change.

### 4. Timeline readability follows the same change

Messages are grouped by local day with a separator (Today, Yesterday, or a date). Conversation previews strip Markdown/HTML and omit the contact's own name for its own direct-conversation replies.

## Alternatives considered

- **Render raw HTML inline without sanitization**: simplest and most flexible, but it executes model output in the app origin. Rejected; the desktop has no CSP to fall back on.
- **Sanitize inline HTML and disable scripts everywhere**: safer, but it cannot provide interactive HTML, which is the product requirement.
- **Render artifacts in a separate top-level window or external browser**: strongest isolation, but loses the in-conversation preview and the ChatGPT-like flow. The sandboxed iframe is the right trade-off.
- **Use `allow-same-origin` to simplify height measurement and storage**: defeats the sandbox, because scripts in an iframe with the same origin can reach the parent and Tauri. Rejected.
- **Allow external images in inline Markdown**: convenient, but a model could trigger network requests and leak the user's IP without a click. Data-only images are the safer default; artifacts use the same rule.

## Consequences

- Agent answers are readable: headings, lists, tables, code, links, and block quotes render.
- Interactive HTML is possible without exposing the app; the boundary is the iframe sandbox plus CSP.
- External images, fonts, network calls, form submissions, and storage are unavailable inside artifacts. Agents are told to use inline CSS/JS and data URIs.
- Sanitization can remove markup an agent expected to keep; formatting that depends on inline `style` attributes is stripped from inline HTML (artifacts keep inline styles inside the iframe).
- The Markdown stack adds roughly 350 KB to the frontend bundle; it is local-disk loaded and split into the main chunk for now.
- No artifact-to-chat bridge, streaming into an artifact, or syntax highlighting.
- Coverage: `pnpm run build` (type-check and bundle) plus manual rendering and security checks; there is no frontend test runner. The manual checks include a Markdown answer, an HTML artifact with a button, and a `<script>` / `onerror` probe that must not escape the iframe.
