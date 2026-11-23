'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadWidget, rowIds, click } = require('./helpers/load-widget');

function fixture() {
  return [
    { id: 1, symbol: 'BTC-USDT', price_decimals: 2, change: 1.5, bid: 8576000, ask: 8576100, high: 8600000 },
    { id: 2, symbol: 'ETH-USDT', price_decimals: 2, change: -2.25, bid: 270400, ask: 270450, high: 272000 },
    { id: 3, symbol: 'BNB-USDT', price_decimals: 2, change: null, bid: null, ask: null, high: null },
  ];
}

async function mounted(t, overrides = {}) {
  const env = loadWidget(['market.js', 'watchlist.js']);
  t.after(() => env.cleanup());
  let data = fixture();
  const selected = { id: null };
  const calls = { select: [], data: [] };
  const wl = env.window.Watchlist.mount(env.root, Object.assign({
    fetch: async () => data,
    instrument: () => selected.id,
    onSelect: (id) => { selected.id = id; calls.select.push(id); },
    onData: (list) => calls.data.push(list),
    interval: 0,
  }, overrides));
  await tick(env);
  return { ...env, wl, calls, selected, setData: (d) => { data = d; } };
}

function tick(env) {
  return new Promise((r) => env.window.setTimeout(r, 0));
}

test('строки строятся из источника, по одной на инструмент', async (t) => {
  const env = await mounted(t);
  assert.deepEqual(rowIds(env.root), [1, 2, 3]);
});

test('первый инструмент выбирается автоматически, но только пока ничего не выбрано', async (t) => {
  const env = await mounted(t);
  assert.deepEqual(env.calls.select, [1]);
  await env.wl.refresh();
  assert.deepEqual(env.calls.select, [1], 'повторный refresh не должен переизбирать инструмент');
});

test('источник вернул null — прежние строки остаются, а не исчезают', async (t) => {
  const env = await mounted(t);
  env.setData(null);
  await env.wl.refresh();
  assert.deepEqual(rowIds(env.root), [1, 2, 3],
    'обрыв связи не должен читаться как «инструментов нет»: это отказ, похожий на данные');
});

test('отсутствующая котировка рисуется прочерком, а не нулём', async (t) => {
  const env = await mounted(t);
  const row = env.root.querySelector('.irow[data-id="3"]');
  assert.equal(row.querySelector('.c-sell').textContent, '—');
  assert.equal(row.querySelector('.c-buy').textContent, '—');
  assert.equal(row.querySelector('.c-high').textContent, '—');
  assert.equal(row.querySelector('.c-change').textContent, '—');
  const chg = row.querySelector('.c-change').classList;
  assert.ok(!chg.contains('up') && !chg.contains('down'), 'без направления не красим ни вверх, ни вниз');
  assert.ok(!/0\.00/.test(row.querySelector('.c-sell').textContent),
    'ноль — правдоподобная цена, по ней принимают решение; прочерк — нет');
});

test('цена форматируется по price_decimals самого инструмента', async (t) => {
  const env = await mounted(t);
  const row = env.root.querySelector('.irow[data-id="1"]');
  assert.equal(row.querySelector('.c-sell').textContent, '85,760.00');
});

test('обновление переиспользует элементы строк, а не пересоздаёт их', async (t) => {
  const env = await mounted(t);
  const before = [...env.root.querySelectorAll('.irow')];
  await env.wl.refresh();
  const after = [...env.root.querySelectorAll('.irow')];
  assert.equal(after.length, 3, 'строк должно остаться столько же, сколько инструментов');
  after.forEach((el, i) => assert.equal(el, before[i],
    'пересоздание строк теряет позицию скролла на каждом тике — дефект, который не падает'));
});

test('избранное поднимается наверх и переживает перемонтирование', async (t) => {
  const env = await mounted(t);
  click(env.window, env.root.querySelector('.irow[data-id="3"] .star'));
  assert.deepEqual(rowIds(env.root), [3, 1, 2]);
  assert.deepEqual(JSON.parse(env.window.localStorage.getItem('watch.favs')), [3]);

  env.wl.destroy();
  const wl2 = env.window.Watchlist.mount(env.root, { fetch: async () => fixture(), interval: 0 });
  await tick(env);
  assert.deepEqual(rowIds(env.root), [3, 1, 2], 'избранное читается из localStorage при монтировании');
  wl2.destroy();
});

test('клик по звезде не выбирает инструмент', async (t) => {
  const env = await mounted(t);
  const before = env.calls.select.length;
  click(env.window, env.root.querySelector('.irow[data-id="2"] .star'));
  assert.equal(env.calls.select.length, before, 'звезда — это избранное, а не выбор инструмента');
});

test('сортировка по колонке и разворот при повторном клике', async (t) => {
  const env = await mounted(t);
  const head = env.root.querySelector('.sortable[data-sort="change"]');
  click(env.window, head);
  assert.deepEqual(rowIds(env.root), [1, 3, 2], 'по убыванию change: 1.5, 0.4, -2.25');
  click(env.window, head);
  assert.deepEqual(rowIds(env.root), [2, 3, 1], 'повторный клик разворачивает порядок');
});

test('инструменты без котировки уходят вниз, а не наверх', async (t) => {
  const env = await mounted(t);
  click(env.window, env.root.querySelector('.sortable[data-sort="sell"]'));
  assert.equal(rowIds(env.root).at(-1), 3, 'пустой bid не должен читаться как самая низкая настоящая цена');
});

test('поиск прячет строки, но не удаляет их', async (t) => {
  const env = await mounted(t);
  const input = env.root.querySelector('.wl-search');
  /** @type {HTMLInputElement} */ (input).value = 'eth';
  input.dispatchEvent(new env.window.Event('input', { bubbles: true }));
  const visible = [...env.root.querySelectorAll('.irow')].filter((el) => /** @type {HTMLElement} */ (el).style.display !== 'none');
  assert.deepEqual(visible.map((el) => Number(/** @type {HTMLElement} */ (el).dataset.id)), [2]);
  assert.equal(env.root.querySelectorAll('.irow').length, 3, 'фильтр не должен перестраивать DOM');
});

test('скрытая колонка запоминается и возвращается при перемонтировании', async (t) => {
  const env = await mounted(t);
  env.wl.setColumns({ spread: 0 });
  assert.equal(env.wl.columns().spread, 0);
  assert.ok(env.root.classList.contains('hide-spread'));

  env.wl.destroy();
  const wl2 = env.window.Watchlist.mount(env.root, { fetch: async () => fixture(), interval: 0 });
  await tick(env);
  assert.equal(wl2.columns().spread, 0, 'видимость колонок читается из localStorage');
  wl2.destroy();
});

test('columns() отдаёт копию: правка результата не меняет виджет', async (t) => {
  const env = await mounted(t);
  const cols = env.wl.columns();
  cols.high = 0;
  assert.equal(env.wl.columns().high, 1, 'внутреннее состояние наружу не отдаётся');
});

test('destroy очищает контейнер и снимает слушатели', async (t) => {
  const env = await mounted(t);
  const rows = env.root.querySelector('.watch__rows');
  env.wl.destroy();
  assert.equal(env.root.innerHTML, '');
  const before = env.calls.select.length;
  click(env.window, rows);
  assert.equal(env.calls.select.length, before);
});

test('destroy останавливает автообновление', async (t) => {
  const env = loadWidget(['market.js', 'watchlist.js']);
  t.after(() => env.cleanup());
  let fetches = 0;
  const wl = env.window.Watchlist.mount(env.root, {
    fetch: async () => { fetches += 1; return fixture(); },
    interval: 5,
  });
  await new Promise((r) => env.window.setTimeout(r, 30));
  const during = fetches;
  assert.ok(during > 1, `таймер обязан был сработать хотя бы раз, было ${during}`);
  wl.destroy();
  await new Promise((r) => env.window.setTimeout(r, 30));
  assert.equal(fetches, during, 'после destroy виджет не должен ходить в сеть');
});

test('инструмент платформы без дефиса: пара из base/quote, имя в подсказке', async (t) => {
  const env = await mounted(t, {
    fetch: async () => [
      { id: 10, symbol: 'EURUSD', base: 'EUR', quote: 'USD', name: 'Euro / US Dollar', price_decimals: 5, change: null, bid: 108412, ask: 108415, high: null },
      { id: 11, symbol: 'AAPL', price_decimals: 2, change: 0.5, bid: 22814, ask: 22816, high: 23000 },
    ],
  });
  const eur = /** @type {HTMLElement} */ (env.root.querySelector('.irow[data-id="10"]'));
  assert.equal(eur.querySelector('.symtxt').textContent, 'EUR/USD');
  assert.equal(eur.title, 'Euro / US Dollar');
  assert.equal(env.root.querySelector('.irow[data-id="11"] .symtxt').textContent, 'AAPL', 'без quote — без «/USDT»');
  const q = /** @type {HTMLInputElement} */ (env.root.querySelector('.wl-search'));
  q.value = 'euro';
  q.dispatchEvent(new env.window.Event('input'));
  assert.equal(eur.style.display, '', 'поиск находит и по имени');
  assert.equal(/** @type {HTMLElement} */ (env.root.querySelector('.irow[data-id="11"]')).style.display, 'none');
});

test('пометка рынка от хоста видна в строке, update применяет смену статуса сразу', async (t) => {
  const note = (it) => (it.market && it.market.status === 'closed' ? { kind: 'closed', text: 'Closed', tip: 'Market closed · opens 22:00' } : null);
  const env = await mounted(t, {
    fetch: async () => [{ id: 10, symbol: 'EURUSD', base: 'EUR', quote: 'USD', price_decimals: 5, change: null, bid: 1, ask: 2, high: null, market: { status: 'open' } }],
    note,
  });
  const row = () => env.root.querySelector('.irow[data-id="10"]');
  assert.equal(row().querySelector('.wl-mk'), null);
  env.wl.update(10, { market: { status: 'closed' } });
  const mk = /** @type {HTMLElement} */ (row().querySelector('.wl-mk'));
  assert.equal(mk.textContent, 'Closed');
  assert.equal(mk.title, 'Market closed · opens 22:00');
  assert.ok(mk.classList.contains('wl-mk--closed'));
  env.wl.update(999, { market: { status: 'closed' } });
});

test('крипто-строка без пометок выглядит как раньше', async (t) => {
  const env = await mounted(t, { note: () => null });
  const row = env.root.querySelector('.irow[data-id="1"]');
  assert.equal(row.querySelector('.symtxt').textContent, 'BTC/USDT');
  assert.equal(row.querySelector('.wl-mk'), null);
});

test('первый выбор берёт инструмент, который назвал хост; дальше опрос не перевыбирает', async (t) => {
  const env = await mounted(t, { initial: () => 2 });
  assert.deepEqual(env.calls.select, [2]);
  await env.wl.refresh();
  assert.deepEqual(env.calls.select, [2]);
});

test('хост назвал инструмент, которого нет в списке, — первый по порядку', async (t) => {
  const env = await mounted(t, { initial: () => 99 });
  assert.deepEqual(env.calls.select, [1]);
});

function mixed() {
  return [
    { id: 1, symbol: 'BTC-USDT', category: 'crypto', price_decimals: 2, change: 1, bid: 1, ask: 2, high: 3 },
    { id: 31, symbol: 'EURUSD', category: 'forex', price_decimals: 5, change: null, bid: 1, ask: 2, high: null },
    { id: 2, symbol: 'ETH-USDT', category: 'crypto', price_decimals: 2, change: 1, bid: 1, ask: 2, high: 3 },
    { id: 32, symbol: 'XAUUSD', category: 'metals', price_decimals: 2, change: null, bid: 1, ask: 2, high: null },
    { id: 40, symbol: 'AAPL', category: 'stocks', price_decimals: 2, change: null, bid: 1, ask: 2, high: null },
  ];
}
/** @param {any} env */
const visibleIds = (env) => [...env.root.querySelectorAll('.irow')].filter((el) => el.style.display !== 'none').map((el) => +el.dataset.id);
/** @param {any} env */
const tabs = (env) => [...env.root.querySelectorAll('.wl-cat')].map((b) => b.dataset.cat + (b.classList.contains('on') ? '*' : ''));

test('категории: вкладки только для имеющихся категорий, «Все» сгруппировано по категориям', async (t) => {
  const env = await mounted(t, { fetch: async () => mixed() });
  assert.deepEqual(tabs(env), ['all*', 'crypto', 'forex', 'metals', 'stocks']);
  assert.deepEqual(visibleIds(env), [1, 2, 31, 32, 40], 'крипто, затем форекс, металлы, акции — не вперемешку');
});

test('категории: вкладка фильтрует, поиск ищет по всем, выбор запоминается', async (t) => {
  const env = await mounted(t, { fetch: async () => mixed() });
  click(env.window, env.root.querySelector('.wl-cat[data-cat="forex"]'));
  assert.deepEqual(visibleIds(env), [31]);
  assert.deepEqual(tabs(env), ['all', 'crypto', 'forex*', 'metals', 'stocks']);
  assert.equal(env.wl.category(), 'forex');
  assert.equal(env.window.localStorage.getItem('watch.cat'), '"forex"');
  const q = /** @type {HTMLInputElement} */ (env.root.querySelector('.wl-search'));
  q.value = 'xau';
  q.dispatchEvent(new env.window.Event('input'));
  assert.deepEqual(visibleIds(env), [32], 'поиск не ограничен вкладкой');
  q.value = '';
  q.dispatchEvent(new env.window.Event('input'));
  assert.deepEqual(visibleIds(env), [31]);
  const wl2 = env.window.Watchlist.mount(env.root, { fetch: async () => mixed(), interval: 0 });
  await tick(env);
  assert.equal(wl2.category(), 'forex', 'после перезагрузки открыта та же вкладка');
  wl2.destroy();
});

test('категории: одна категория — вкладок нет; пропавшая категория возвращает на «Все»', async (t) => {
  const env = await mounted(t);
  assert.equal(/** @type {HTMLElement} */ (env.root.querySelector('.watch__cats')).hidden, true, 'только крипто — вкладки не нужны');
  assert.deepEqual(visibleIds(env), [1, 2, 3]);
  env.setData(mixed());
  await env.wl.refresh();
  env.wl.setCategory('metals');
  assert.deepEqual(visibleIds(env), [32]);
  env.setData(mixed().filter((it) => it.category !== 'metals'));
  await env.wl.refresh();
  assert.equal(env.wl.category(), 'all');
  assert.deepEqual(visibleIds(env), [1, 2, 31, 40], 'строки пропавших инструментов убраны');
});

test('категории: внутри категории порядок источника (CMS), а не номер инструмента', async (t) => {
  const env = await mounted(t, { fetch: async () => [
    { id: 32, symbol: 'XAUUSD', category: 'metals', price_decimals: 2, change: null, bid: 1, ask: 2, high: null },
    { id: 7, symbol: 'ADA-USDT', category: 'crypto', price_decimals: 4, change: 1, bid: 1, ask: 2, high: 3 },
    { id: 33, symbol: 'GBPUSD', category: 'forex', price_decimals: 5, change: null, bid: 1, ask: 2, high: null },
    { id: 1, symbol: 'BTC-USDT', category: 'crypto', price_decimals: 2, change: 1, bid: 1, ask: 2, high: 3 },
    { id: 31, symbol: 'EURUSD', category: 'forex', price_decimals: 5, change: null, bid: 1, ask: 2, high: null },
  ] });
  assert.deepEqual(visibleIds(env), [7, 1, 33, 31, 32]);
});

test('категории: у вкладки счётчик инструментов, вкладки переносятся, а не прячутся за край', async (t) => {
  const env = await mounted(t);
  env.setData(mixed());
  await env.wl.refresh();
  const counts = [...env.root.querySelectorAll('.wl-cat')].map((b) => `${/** @type {HTMLElement} */ (b).dataset.cat}:${b.querySelector('.wl-cat__n')?.textContent}`);
  assert.deepEqual(counts, ['all:5', 'crypto:2', 'forex:1', 'metals:1', 'stocks:1']);
  const css = env.window.document.getElementById('watchlist-css')?.textContent || '';
  const rule = (css.match(/\.watch__cats \{[^}]*\}/) || [''])[0];
  assert.match(rule, /flex-wrap: wrap/, 'вкладки переносятся на вторую строку');
  assert.doesNotMatch(rule, /overflow-x/, 'нет горизонтальной прокрутки, где последняя вкладка обрезана');
  assert.doesNotMatch(css, /:has\(/, 'без :has() — Chrome 104');
  env.setData([...mixed(), { id: 41, symbol: 'TSLA', category: 'stocks', price_decimals: 2, change: null, bid: 1, ask: 2, high: null }]);
  await env.wl.refresh();
  assert.equal(env.root.querySelector('.wl-cat[data-cat="stocks"] .wl-cat__n')?.textContent, '2', 'счётчик обновляется с новым списком');
});

test('категории: вкладки доступны с клавиатуры (role=tab, стрелки, Home/End)', async (t) => {
  const env = await mounted(t, { fetch: async () => mixed() });
  const list = /** @type {HTMLElement} */ (env.root.querySelector('.watch__cats'));
  assert.equal(list.getAttribute('role'), 'tablist');
  assert.ok(list.getAttribute('aria-label'));
  const key = (k) => list.dispatchEvent(new /** @type {any} */ (env.window).KeyboardEvent('keydown', { key: k, bubbles: true }));
  key('ArrowRight');
  assert.equal(env.wl.category(), 'crypto');
  const on = /** @type {HTMLElement} */ (env.root.querySelector('.wl-cat.on'));
  assert.equal(on.getAttribute('aria-selected'), 'true');
  assert.equal(on.getAttribute('tabindex'), '0');
  key('End');
  assert.equal(env.wl.category(), 'stocks');
  key('ArrowRight');
  assert.equal(env.wl.category(), 'all', 'по кругу');
  key('ArrowLeft');
  assert.equal(env.wl.category(), 'stocks');
  key('Home');
  assert.equal(env.wl.category(), 'all');
  assert.deepEqual(visibleIds(env), [1, 2, 31, 32, 40]);
});
