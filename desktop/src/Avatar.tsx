import type { CSSProperties } from "react";

const palette = [
  "#c9a66b",
  "#9aa88f",
  "#c98f6b",
  "#8fa3b8",
  "#b28fb0",
  "#7fa8a0",
  "#c7b07a",
  "#a89880",
];

function hashName(name: string): number {
  let hash = 0;
  for (let index = 0; index < name.length; index += 1) {
    hash = (hash * 31 + name.charCodeAt(index)) >>> 0;
  }
  return hash;
}

function initials(name: string): string {
  const words = name.trim().split(/\s+/).filter(Boolean);
  if (words.length === 0) return "?";
  if (words.length === 1) return words[0].slice(0, 1).toUpperCase();
  return `${words[0].slice(0, 1)}${words[1].slice(0, 1)}`.toUpperCase();
}

interface AvatarProps {
  name: string;
  kind: "agent" | "channel" | "user";
  size?: "small" | "medium" | "large";
}

export default function Avatar({ name, kind, size = "medium" }: AvatarProps) {
  const color = palette[hashName(name) % palette.length];
  const label = kind === "channel" ? "#" : initials(name);
  const style = {
    color,
    backgroundColor: `${color}26`,
    borderColor: `${color}66`,
  } as CSSProperties;

  return (
    <span
      className={`avatar avatar-${kind} avatar-${size}`}
      style={style}
      aria-hidden="true"
    >
      {label}
    </span>
  );
}
