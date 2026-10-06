'use strict';
// ============================================================================
// Загрузка ВИДЖЕТА терминала в Node с настоящим DOM.
//
// Чем отличается от `load-module.js`: тот даёт модулю фиктивный `window` и годится
// для чистых функций (индикаторы). Виджет по контракту ADR-015 строит свой DOM
// внутри `root`, вешает слушатели и пишет в `localStorage` — подделать это
// заглушкой нельзя, не превратив тест в проверку заглушки.
//
// jsdom — **только devDependency** (решение владельца Р-020: «Разрешаем только
// для разработки»). В поставку не входит: образ собирается из `public/` как есть,
// npm в Dockerfile не участвует. Позиция «ноль зависимостей в рантайме» цела —
// ценной была она, а не запрет на инструменты разработки.
//
// ⚠️ Объекты из jsdom принадлежат **другому realm**: `deepStrictEqual` падает на
// прототипе при совпадающем содержимом (та же грабля, что у vm-песочницы).
// Сравнивать поэлементно или через `JSON.parse(JSON.stringify(...))`.
// ============================================================================
const fs = require('node:fs');
const path = require('node:path');
const { JSDOM } = require('jsdom');

const PUBLIC_DIR = path.join(__dirname, '..', '..', 'public');

/**
 * Поднять jsdom и выполнить в нём модули виджетов.
 *
 * @param {string[]} files имена файлов из public/, например ['watchlist.js']
 * @returns {{ window: Window, document: Document, root: Element, cleanup: () => void }}
 */
function loadWidget(files) {
  const dom = new JSDOM('<!doctype html><html><head></head><body><div id="root"></div></body></html>', {
    // url обязателен: без origin'а jsdom не даёт localStorage, а виджет пишет в него
    // избранное и видимость колонок — то самое, что мы и хотим проверить.
    url: 'http://localhost:8888/',
    pretendToBeVisual: true,
    // outside-only: скрипты со страницы не исполняются (нам нечего грузить), но
    // `window.eval` получает контекст окна — без этого модуль не видит `document`.
    runScripts: 'outside-only',
  });
  for (const file of files) {
    const code = fs.readFileSync(path.join(PUBLIC_DIR, file), 'utf8');
    dom.window.eval(code);
  }
  return {
    window: dom.window,
    document: dom.window.document,
    root: dom.window.document.getElementById('root'),
    cleanup: () => dom.window.close(),
  };
}

/** Текст строк виджета в порядке их следования в DOM. */
function rowIds(root) {
  return [...root.querySelectorAll('.irow')].map((el) => Number(el.dataset.id));
}

/** Клик, который видят делегированные слушатели (нужен bubbles). */
function click(window, el) {
  el.dispatchEvent(new window.MouseEvent('click', { bubbles: true }));
}

module.exports = { loadWidget, rowIds, click };
