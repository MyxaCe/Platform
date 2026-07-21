/* ============================================================================
 * i18n.js — механизм переключения языка. ОБЩИЙ модуль (ADR-017): сам словарь
 * у каждого продукта свой, здесь только машинерия.
 *
 *   I18n.init({ en: {...}, ru: {...} }, { key: 'site.lang', fallback: 'en' });
 *   I18n.setLang('ru');  I18n.t('nav.markets');  I18n.apply();
 *
 * Тексты в разметке помечаются data-i18n (текст) и data-i18n-ph (placeholder).
 * ========================================================================== */
(function () {
  let DICT = {};
  let lang = 'en';
  let fallback = 'en';
  let storeKey = 'app.lang';
  const listeners = [];

  const t = (key) => (DICT[lang] && DICT[lang][key]) || (DICT[fallback] && DICT[fallback][key]) || key;

  /** Проставить переводы в разметке. Вызывается после каждой смены языка. */
  function apply(root = document) {
    root.querySelectorAll('[data-i18n]').forEach((el) => { el.textContent = t(el.dataset.i18n); });
    root.querySelectorAll('[data-i18n-ph]').forEach((el) => { el.placeholder = t(el.dataset.i18nPh); });
    document.documentElement.lang = lang;
  }

  function setLang(next) {
    if (!DICT[next] || next === lang) return;
    lang = next;
    localStorage.setItem(storeKey, lang);
    apply();
    listeners.forEach((fn) => fn(lang));
  }

  /** Продукт передаёт свой словарь и свой ключ хранения. */
  function init(dicts, opts = {}) {
    DICT = dicts || {};
    fallback = opts.fallback || 'en';
    storeKey = opts.key || 'app.lang';
    const saved = localStorage.getItem(storeKey);
    lang = DICT[saved] ? saved : fallback;
    apply();
  }

  window.I18n = { init, t, apply, setLang, lang: () => lang, onChange: (fn) => listeners.push(fn) };
})();
