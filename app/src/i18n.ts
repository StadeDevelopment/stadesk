import { createContext, useContext } from "react";
import en from "./i18n/en.json";
import tr from "./i18n/tr.json";

export type Language = "en" | "tr";
export type Key = keyof typeof en;

const tables: Record<Language, Record<string, string>> = { en, tr };

export function systemLanguage(): Language {
  const preferred = navigator.languages?.length ? navigator.languages : [navigator.language];
  return preferred.some((tag) => tag?.toLowerCase().startsWith("tr")) ? "tr" : "en";
}

export function resolveLanguage(setting: string | undefined): Language {
  if (setting === "en" || setting === "tr") {
    return setting;
  }
  return systemLanguage();
}

export function translate(language: Language, key: string, values: Record<string, string> = {}): string {
  const template = tables[language][key] ?? tables.en[key] ?? key;
  return template.replace(/\{(\w+)\}/g, (match, name: string) => values[name] ?? match);
}

export const LanguageContext = createContext<Language>("en");

export function useT() {
  const language = useContext(LanguageContext);
  return (key: Key, values?: Record<string, string>) => translate(language, key, values);
}

export function useErrorText() {
  const language = useContext(LanguageContext);
  return (error: { code: string; detail?: string | null }) => {
    const key = `error.${error.code}`;
    const known = key in tables.en;
    return translate(language, known ? key : "error.unknown", {
      detail: error.detail ?? error.code,
    });
  };
}
