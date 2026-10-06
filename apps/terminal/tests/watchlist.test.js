'use strict';
// ============================================================================
// Тесты виджета watchlist (TRM-05, контракт ADR-015 `mount(root, ctx)`).
//
// Почему именно эти проверки: ошибка в виджете не падает, а рисует неправильное.
// Три давних дефекта UI (BUG-008…010) нашлись только при рефакторинге и жили до
// него месяцами. Поэтому здесь закрепляется не «виджет смонтировался», а
// поведение, которое ломается молча:
//   • отказ источника не должен выглядеть пустым списком (BUG-011);
//   • отсутствующая котировка рисуется прочерком, а не нулём (КОНТРАКТЫ §5г);
//   • строки переиспользуются, а не пересоздаются — иначе теряется скролл;
//   • destroy действительно снимает слушатели и таймер.
// ============================================================================
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadWidget, rowIds, click } = require('./helpers/load-widget');

/** Инструменты: цена и растёт и падает, есть неполная котировка — ломать есть чем. */
function fixture() {
  return [
    { id: 1, symbol: 'BTC-USDT', price_decimals: 2, change: 1.5, bid: 8576000, ask: 8576100, high: 8600000 },
    { id: 2, symbol: 'ETH-USDT', price_decimals: 2, change: -2.25, bid: 270400, ask: 270450, high: 272000 },
    { id: 3, symbol: 'BNB-USDT', price_decimals: 2, change: 0.4, bid: null, ask: null, high: null },
  ];
}

/**
 * Смонтировать виджет и дождаться первого refresh (mount зовёт его без await).
 *
 * `t.after` обязателен: если assert упал, `cleanup()` в конце теста не выполнится,
 * а невыключенный таймер виджета держит event loop — прогон **виснет вместо того,
 * чтобы покраснеть**. Молчаливое зависание хуже красного теста: его списывают на
 * «что-то долго». Поймано мутацией «destroy не снимает таймер».
 */
async function mounted(t, overrides = {}) {
  const env = loadWidget(['watchlist.js']);
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

// ---- Отказ источника не выглядит данными (BUG-011) -------------------------

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
  assert.ok(!/0\.00/.test(row.querySelector('.c-sell').textContent),
    'ноль — правдоподобная цена, по ней принимают решение; прочерк — нет');
});

test('цена форматируется по price_decimals самого инструмента', async (t) => {
  const env = await mounted(t);
  const row = env.root.querySelector('.irow[data-id="1"]');
  assert.equal(row.querySelector('.c-sell').textContent, '85,760.00');
});

// ---- Переиспользование строк -----------------------------------------------

test('обновление переиспользует элементы строк, а не пересоздаёт их', async (t) => {
  const env = await mounted(t);
  const before = [...env.root.querySelectorAll('.irow')];
  await env.wl.refresh();
  const after = [...env.root.querySelectorAll('.irow')];
  // Число строк проверяется первым и не зря: при пересоздании старый элемент из
  // DOM не удаляется, `querySelector` по-прежнему находит его, и проверка одной
  // лишь идентичности оставалась бы зелёной. Мутация «пересоздавать строки»
  // ловится именно ростом их количества.
  assert.equal(after.length, 3, 'строк должно остаться столько же, сколько инструментов');
  after.forEach((el, i) => assert.equal(el, before[i],
    'пересоздание строк теряет позицию скролла на каждом тике — дефект, который не падает'));
});

// ---- Избранное --------------------------------------------------------------

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

// ---- Сортировка -------------------------------------------------------------

test('сортировка по колонке и разворот при повторном клике', async (t) => {
  const env = await mounted(t);
  const head = env.root.querySelector('.sortable[data-sort="change"]');
  click(env.window, head);
  assert.deepEqual(rowIds(env.root), [1, 3, 2], 'по убыванию change: 1.5, 0.4, -2.25');
  click(env.window, head);
  assert.deepEqual(rowIds(env.root), [2, 3, 1], 'повторный клик разворачивает порядок');
});

// ⚠️ Мутация `it.bid ?? -Infinity` → `it.bid ?? 0` этот тест **не красит**, и это
// не дыра: цены неотрицательны, поэтому ноль и минус бесконечность дают один и тот
// же порядок. Выбор `-Infinity` защищает от величин, которые могут быть
// отрицательными, а у `bid`/`ask`/`high` таких не бывает по построению. Записано,
// чтобы следующий не «дозакрывал» непокрытую мутацию ради полноты.
test('инструменты без котировки уходят вниз, а не наверх', async (t) => {
  const env = await mounted(t);
  click(env.window, env.root.querySelector('.sortable[data-sort="sell"]'));
  assert.equal(rowIds(env.root).at(-1), 3, 'пустой bid не должен читаться как самая низкая настоящая цена');
});

// ---- Поиск ------------------------------------------------------------------

test('поиск прячет строки, но не удаляет их', async (t) => {
  const env = await mounted(t);
  const input = env.root.querySelector('.wl-search');
  input.value = 'eth';
  input.dispatchEvent(new env.window.Event('input', { bubbles: true }));
  const visible = [...env.root.querySelectorAll('.irow')].filter((el) => el.style.display !== 'none');
  assert.deepEqual(visible.map((el) => Number(el.dataset.id)), [2]);
  assert.equal(env.root.querySelectorAll('.irow').length, 3, 'фильтр не должен перестраивать DOM');
});

// ---- Колонки ----------------------------------------------------------------

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

// ---- destroy ----------------------------------------------------------------

test('destroy очищает контейнер и снимает слушатели', async (t) => {
  const env = await mounted(t);
  const rows = env.root.querySelector('.watch__rows');
  env.wl.destroy();
  assert.equal(env.root.innerHTML, '');
  const before = env.calls.select.length;
  click(env.window, rows); // элемент уже отсоединён, слушателя быть не должно
  assert.equal(env.calls.select.length, before);
});

test('destroy останавливает автообновление', async (t) => {
  const env = loadWidget(['watchlist.js']);
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
