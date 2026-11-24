import i18n from 'i18next';
import { initReactI18next } from 'react-i18next';
import LanguageDetector from 'i18next-browser-languagedetector';
import en from './locales/en.json';
import { proseValue } from '../utils/proseTokenCore';

const isDev = import.meta.env.DEV;

void i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources: { en: { translation: en } },
    fallbackLng: 'en',
    // No HTML escaping (React escapes), but every interpolated value is made
    // safe for a prose sentence (RS-PROSE fix 1): this page's own name
    // tokens are kept, and any other U+0001/U+0002 marker or C0 control in a
    // comment, a name or a file name is dropped, so no value can forge a
    // pill or crash one. It is done in `format` (run for every value with
    // `alwaysFormat`), not in `escape`: i18next moves its placeholder cursor
    // by the length of the value `format` returns, and an `escape` that
    // shortened it made i18next skip the next placeholder (a literal
    // `{{b}}` shown). No string uses a `{{x, format}}` formatter, which this
    // `format` replaces.
    interpolation: {
      escapeValue: false,
      alwaysFormat: true,
      // Typed `string`, but any other value (a number, a missing variable's
      // undefined) must go back as it came: i18next handles those itself.
      format: (value: unknown) => (typeof value === 'string' ? proseValue(value) : value) as string,
    },
    returnNull: false,
    returnEmptyString: false,
    saveMissing: true,
    parseMissingKeyHandler: isDev ? (key) => `[${key}]` : () => '',
    missingKeyHandler: (_lngs, _ns, key) => {
      if (isDev) {
        console.warn(`[i18n] Missing key: ${key}`);
      } else {
        console.warn(`[i18n] Missing key in production: ${key}`);
      }
    },
    detection: {
      order: ['navigator'],
      caches: [],
    },
  });

export default i18n;
