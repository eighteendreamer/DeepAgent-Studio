import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';

import en from './locales/en.json';
import zh from './locales/zh.json';
import sq from './locales/sq.json';
import is from './locales/is.json';
import ka from './locales/ka.json';
import mk from './locales/mk.json';
import mn from './locales/mn.json';
import my from './locales/my.json';
import ja from './locales/ja.json';
import so from './locales/so.json';
import hy from './locales/hy.json';
import zhTW from './locales/zh-TW.json';
import zhHK from './locales/zh-HK.json';

const resources = {
  en: { translation: en },
  zh: { translation: zh },
  sq: { translation: sq },
  is: { translation: is },
  ka: { translation: ka },
  mk: { translation: mk },
  mn: { translation: mn },
  my: { translation: my },
  ja: { translation: ja },
  so: { translation: so },
  hy: { translation: hy },
  'zh-TW': { translation: zhTW },
  'zh-HK': { translation: zhHK }
};

const savedLanguage = localStorage.getItem('appLanguage') || 'zh';

i18n
  .use(initReactI18next)
  .init({
    resources,
    lng: savedLanguage,
    fallbackLng: 'zh',
    interpolation: {
      escapeValue: false // React already does escaping
    }
  });

export default i18n;
