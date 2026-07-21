/* ============================================================================
 * theme.js — светлая/тёмная тема. ОБЩИЙ модуль: используется сайтом и кабинетом
 * (ADR-017). Ключ хранения задаётся продуктом, чтобы они не перетирали выбор
 * друг другу. Тема = атрибут data-theme на <html>, все цвета
 * в CSS берутся из токенов, поэтому переключение не трогает разметку.
 *
 * Приоритет: выбор пользователя (localStorage) → системная настройка → тёмная.
 * ========================================================================== */
(function () {
  const KEY = (window.__themeKey || 'app') + '.theme';

  const system = () => (window.matchMedia && window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark');
  let theme = localStorage.getItem(KEY) || system();

  function set(next) {
    theme = next === 'light' ? 'light' : 'dark';
    document.documentElement.dataset.theme = theme;
    localStorage.setItem(KEY, theme);
  }

  set(theme); // до первой отрисовки, чтобы не мигало

  window.Theme = {
    get: () => theme,
    set,
    toggle: () => set(theme === 'dark' ? 'light' : 'dark'),
  };
})();
