/* ============================================================================
 * theme.js — светлая/тёмная тема. Тема = атрибут data-theme на <html>, все цвета
 * в CSS берутся из токенов, поэтому переключение не трогает разметку.
 *
 * Приоритет: выбор пользователя (localStorage) → системная настройка → тёмная.
 * ========================================================================== */
(function () {
  const KEY = 'site.theme';

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
