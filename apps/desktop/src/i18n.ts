import i18n from "i18next";
import type { Resource } from "i18next";
import { initReactI18next } from "react-i18next";

import zh from "./locales/zh.json";

const FALLBACK_LANGUAGE = "zh";

const localeLoaders = {
  en: () => import("./locales/en.json"),
  sq: () => import("./locales/sq.json"),
  is: () => import("./locales/is.json"),
  ka: () => import("./locales/ka.json"),
  mk: () => import("./locales/mk.json"),
  mn: () => import("./locales/mn.json"),
  my: () => import("./locales/my.json"),
  ja: () => import("./locales/ja.json"),
  so: () => import("./locales/so.json"),
  hy: () => import("./locales/hy.json"),
  "zh-TW": () => import("./locales/zh-TW.json"),
  "zh-HK": () => import("./locales/zh-HK.json"),
} as const;

type DynamicLanguage = keyof typeof localeLoaders;
type SupportedLanguage = typeof FALLBACK_LANGUAGE | DynamicLanguage;

function normalizeLanguage(value: string | null): SupportedLanguage {
  if (value === FALLBACK_LANGUAGE) return FALLBACK_LANGUAGE;
  return value && value in localeLoaders ? (value as DynamicLanguage) : FALLBACK_LANGUAGE;
}

async function loadLanguage(language: SupportedLanguage) {
  if (language === FALLBACK_LANGUAGE) return;
  if (i18n.hasResourceBundle(language, "translation")) return;
  const module = await localeLoaders[language]();
  i18n.addResourceBundle(language, "translation", module.default, true, true);
}

const savedLanguage = normalizeLanguage(localStorage.getItem("appLanguage"));
const resources: Resource = {
  [FALLBACK_LANGUAGE]: { translation: zh },
};

i18n.use(initReactI18next).init({
  resources,
  lng: FALLBACK_LANGUAGE,
  fallbackLng: FALLBACK_LANGUAGE,
  interpolation: {
    escapeValue: false,
  },
});

const originalChangeLanguage = i18n.changeLanguage.bind(i18n);
i18n.changeLanguage = async (lng?: string, callback?: Parameters<typeof i18n.changeLanguage>[1]) => {
  const normalized = normalizeLanguage(lng ?? savedLanguage);
  await loadLanguage(normalized);
  return originalChangeLanguage(normalized, callback);
};

if (savedLanguage !== FALLBACK_LANGUAGE) {
  void i18n.changeLanguage(savedLanguage);
}

export default i18n;
