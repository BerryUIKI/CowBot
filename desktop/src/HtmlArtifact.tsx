import { useMemo, useState } from "react";

import Modal from "./Modal";

const CSP =
  "default-src 'none'; style-src 'unsafe-inline'; script-src 'unsafe-inline'; " +
  "img-src data:; font-src data:; connect-src 'none'; form-action 'none'; " +
  "base-uri 'none'; frame-src 'none'; object-src 'none'";

const BASE_STYLE =
  "html,body{margin:0;padding:12px;font-family:'Segoe UI Variable Text','Segoe UI',system-ui,sans-serif;font-size:14px;line-height:1.5;color:#1f1f1f;background:#fff;}" +
  "*,*::before,*::after{box-sizing:border-box;}";

/** Wrap agent HTML with a strict CSP; full documents get the meta injected. */
function artifactDocument(html: string): string {
  const meta = `<meta http-equiv="Content-Security-Policy" content="${CSP}">`;
  const cleaned = html
    .replace(/<base\b[^>]*>/gi, "")
    .replace(/<meta[^>]+http-equiv=["']?refresh["']?[^>]*>/gi, "");
  if (/<head[\s>]/i.test(cleaned)) {
    return cleaned.replace(/<head([^>]*)>/i, `<head$1>${meta}`);
  }
  return (
    "<!doctype html><html><head><meta charset=\"utf-8\">" +
    meta +
    '<meta name="viewport" content="width=device-width, initial-scale=1">' +
    `<style>${BASE_STYLE}</style></head><body>${cleaned}</body></html>`
  );
}

interface HtmlArtifactProps {
  html: string;
}

export default function HtmlArtifact({ html }: HtmlArtifactProps) {
  const [mode, setMode] = useState<"preview" | "code">("preview");
  const [expanded, setExpanded] = useState(false);
  const document = useMemo(() => artifactDocument(html), [html]);

  const frame = (height: string) => (
    <iframe
      className="html-artifact-frame"
      style={{ height }}
      title="HTML preview"
      sandbox="allow-scripts allow-forms"
      referrerPolicy="no-referrer"
      srcDoc={document}
    />
  );

  return (
    <div className="html-artifact">
      <div className="html-artifact-head">
        <span className="html-artifact-label">HTML preview</span>
        <div className="html-artifact-actions">
          <button
            type="button"
            aria-pressed={mode === "preview"}
            onClick={() => setMode("preview")}
          >
            Preview
          </button>
          <button
            type="button"
            aria-pressed={mode === "code"}
            onClick={() => setMode("code")}
          >
            Code
          </button>
          <button type="button" onClick={() => setExpanded(true)}>
            Expand
          </button>
        </div>
      </div>
      {mode === "preview" ? (
        frame("340px")
      ) : (
        <pre className="html-artifact-code">
          <code>{html}</code>
        </pre>
      )}
      {expanded ? (
        <Modal title="HTML preview" onClose={() => setExpanded(false)}>
          {frame("70vh")}
        </Modal>
      ) : null}
    </div>
  );
}
