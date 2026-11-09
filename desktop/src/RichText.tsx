import { isValidElement } from "react";
import type { ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import Markdown from "react-markdown";
import rehypeRaw from "rehype-raw";
import rehypeSanitize, { defaultSchema } from "rehype-sanitize";
import remarkGfm from "remark-gfm";

import HtmlArtifact from "./HtmlArtifact";

const sanitizeSchema = {
  ...defaultSchema,
  tagNames: [
    ...(defaultSchema.tagNames ?? []),
    "table",
    "thead",
    "tbody",
    "tr",
    "th",
    "td",
    "img",
  ],
  attributes: {
    ...defaultSchema.attributes,
    a: [...(defaultSchema.attributes?.a ?? []), "href", "title"],
    img: [...(defaultSchema.attributes?.img ?? []), "src", "alt", "title", "width", "height"],
  },
  protocols: {
    ...defaultSchema.protocols,
    href: ["http", "https", "mailto"],
    src: ["data"],
  },
};

function externalHref(href: string | undefined): string | null {
  return href && /^(https?:|mailto:)/i.test(href) ? href : null;
}

function languageOf(className: unknown): string {
  return typeof className === "string"
    ? className.replace(/^language-/, "").toLowerCase()
    : "";
}

/** Flatten a React node tree to its text content. */
function textOf(node: ReactNode): string {
  if (node === null || node === undefined || typeof node === "boolean") return "";
  if (typeof node === "string" || typeof node === "number") return String(node);
  if (Array.isArray(node)) return node.map(textOf).join("");
  if (isValidElement(node)) {
    return textOf((node.props as { children?: ReactNode }).children);
  }
  return "";
}

interface RichTextProps {
  content: string;
  className?: string;
}

/** Render agent and user output as Markdown, with sanitized HTML and artifacts. */
export default function RichText({ content, className }: RichTextProps) {
  return (
    <div className={`rich-text${className ? ` ${className}` : ""}`}>
      <Markdown
        remarkPlugins={[remarkGfm]}
        rehypePlugins={[rehypeRaw, [rehypeSanitize, sanitizeSchema]]}
        components={{
          a({ href, children }) {
            const external = externalHref(href);
            return (
              <a
                href={href}
                target="_blank"
                rel="noreferrer noopener"
                onClick={(event) => {
                  if (!external) return;
                  event.preventDefault();
                  void openUrl(external);
                }}
              >
                {children}
              </a>
            );
          },
          pre({ children }) {
            const child: ReactNode = Array.isArray(children)
              ? children.find((node) => isValidElement(node))
              : children;
            if (isValidElement(child)) {
              const props = child.props as { className?: string; children?: ReactNode };
              const language = languageOf(props.className);
              if (language === "html" || language === "htm") {
                const html = textOf(props.children).replace(/\n$/, "");
                return <HtmlArtifact html={html} />;
              }
            }
            return <pre>{children}</pre>;
          },
        }}
      >
        {content}
      </Markdown>
    </div>
  );
}
