import { t } from "./i18n/catalog";

export function errorMessage(error: unknown): string {
  if (typeof error === "string") return error;
  if (error instanceof Error) return error.message;
  return t("errors.unexpected");
}
