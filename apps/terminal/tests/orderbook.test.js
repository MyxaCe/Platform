'use strict';
// ============================================================================
// Биржевой стакан (TRM-05, ADR-015).
//
// Здесь есть арифметика, которая не падает: группировка уровней по шагу цены.
// Ошибка в ней рисует стакан, которого нет, — и по нему ставят лимитные заявки.
//
// Поэтому группировка проверяется НЕ сравнением с тем, что программа выдаёт
// сегодня, а двумя свойствами, выведенными из определения:
//
//   1) объём сохраняется: группировка перекладывает ликвидность по корзинам,
//      а не создаёт и не теряет её;
//   2) направление округления консервативно: корзина покупок не может стоить
//      ДОРОЖЕ самой дорогой заявки в ней, корзина продаж — ДЕШЕВЛЕ самой
//      дешёвой. Обратное округление показало бы цену лучше существующей, то
//      есть ликвидность, которой нет.
//
// Второе и есть «определяющее свойство»: числа фиксируют сегодняшний выбор,
// свойство — смысл. Систематическое округление в одну сторону здесь не потеря
// точности, а искажение картины рынка.
// ============================================================================
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadWidget, click } = require('./helpers/load-widget');

const settle = () => new Promise((r) => setImmediate(r));

/** Стакан в raw-единицах, как его отдаёт gateway. */
function depth(over) {
  return Object.assign(
    {
      price_decimals: 2,
      qty_decimals: 5,
      // [цена, объём]; лучший бид первым, лучший аск первым
      bids: [[1000000, 200000], [999900, 100000], [999800, 300000]],
      asks: [[1000100, 150000], [1000200, 250000], [1000300, 50000]],
    },
    over || {},
  );
}

function setup() {
  return loadWidget(['orderbook.js']);
}

function mount(env, over) {
  const calls = { fetch: [], pick: [] };
  // Источник оборачивается, а не подменяется: подстановка своего `fetch` через
  // `over` раньше выключала учёт вызовов, и проверка «настройка перечитывает
  // стакан» молча проверяла пустой список. Учёт обязан пережить подмену.
  const inner = (over && over.fetch) || (async () => depth());
  const ctx = Object.assign(
    {
      instrument: () => 1,
      onPick: (p) => calls.pick.push(p),
      symbols: () => ({ base: 'BTC', quote: 'USD' }),
      markPrice: () => null,
      levels: 20,
      interval: 0,
    },
    over || {},
    { fetch: async (id, n) => { calls.fetch.push([id, n]); return inner(id, n); } },
  );
  return { book: env.window.OrderBook.mount(env.root, ctx), calls };
}

const side = (root, cls) => [...root.querySelectorAll(`.${cls} .brow`)].map((r) => ({
  price: r.querySelector('.bp').textContent,
  qty: r.querySelector('.bq').textContent,
  total: r.querySelector('.bt').textContent,
}));

// ---- Группировка: свойства, а не снимок -----------------------------------

test('группировка сохраняет объём: ликвидность перекладывается, а не исчезает', async (t) => {
  const env = setup();
  t.after(env.cleanup);

  // Шаг 100 raw-единиц = 1.00 в цене: три уровня бидов сливаются в один-два.
  env.window.localStorage.setItem('ob.settings', JSON.stringify({ rounding: true, groupMul: 100 }));
  mount(env);
  await settle();

  const sum = (rows) => rows.reduce((s, r) => s + Number(r.qty), 0);
  const d = depth();
  const raw = (list) => list.reduce((s, [, q]) => s + q, 0) / 10 ** d.qty_decimals;

  assert.ok(Math.abs(sum(side(env.root, 'bids')) - raw(d.bids)) < 1e-9, 'объём бидов сохранён');
  assert.ok(Math.abs(sum(side(env.root, 'asks')) - raw(d.asks)) < 1e-9, 'объём асков сохранён');
});

test('группировка не показывает цену лучше существующей', async (t) => {
  // Корзина покупок не может стоить дороже самой дорогой заявки в ней, корзина
  // продаж — дешевле самой дешёвой. Иначе стакан обещает ликвидность, которой
  // нет, и по этой цене ставят заявку.
  const env = setup();
  t.after(env.cleanup);

  env.window.localStorage.setItem('ob.settings', JSON.stringify({ rounding: true, groupMul: 100 }));
  mount(env);
  await settle();

  const d = depth();
  const bestBid = Math.max(...d.bids.map(([p]) => p)) / 100;
  const bestAsk = Math.min(...d.asks.map(([p]) => p)) / 100;

  for (const r of side(env.root, 'bids')) {
    assert.ok(Number(r.price) <= bestBid, `бид ${r.price} дороже лучшей заявки ${bestBid}`);
  }
  for (const r of side(env.root, 'asks')) {
    assert.ok(Number(r.price) >= bestAsk, `аск ${r.price} дешевле лучшей заявки ${bestAsk}`);
  }
});

test('без группировки уровни остаются как есть', async (t) => {
  const env = setup();
  t.after(env.cleanup);

  env.window.localStorage.setItem('ob.settings', JSON.stringify({ rounding: true, groupMul: 1 }));
  mount(env);
  await settle();

  assert.deepEqual(side(env.root, 'bids').map((r) => r.price), ['10000.00', '9999.00', '9998.00']);
});

test('биды идут по убыванию, аски — по возрастанию к середине', async (t) => {
  // Лучшая цена обязана стоять вплотную к середине: по соседней строке
  // отсчитывают расстояние до рынка.
  const env = setup();
  t.after(env.cleanup);
  mount(env);
  await settle();

  const bids = side(env.root, 'bids').map((r) => Number(r.price));
  const asks = side(env.root, 'asks').map((r) => Number(r.price));

  assert.deepEqual(bids, [...bids].sort((a, b) => b - a), 'биды сверху вниз дешевеют');
  // Аски печатаются перевёрнутыми: худшая цена сверху, лучшая внизу у середины.
  assert.deepEqual(asks, [...asks].sort((a, b) => b - a), 'аски сверху вниз дешевеют');
  assert.ok(asks[asks.length - 1] > bids[0], 'лучший аск дороже лучшего бида');
});

// ---- Середина --------------------------------------------------------------

test('середина и спред считаются из лучших заявок', async (t) => {
  const env = setup();
  t.after(env.cleanup);
  mount(env);
  await settle();

  // bb = 10000.00, ba = 10001.00
  assert.equal(env.root.querySelector('.ob__last').textContent, '10000.50');
  assert.equal(env.root.querySelector('.ob__spread').textContent, 'spread 1.00');
});

test('направление цены липкое: цвет держится, пока цена не изменится', async (t) => {
  // Между опросами цена часто совпадает. Если снимать класс на каждом
  // совпадении, строка будет белой почти всегда и перестанет что-либо значить.
  const env = setup();
  t.after(env.cleanup);

  let d = depth();
  const { book } = mount(env, { fetch: async () => d });
  await settle();

  d = depth({ bids: [[1000200, 200000]], asks: [[1000400, 150000]] }); // середина выросла
  await book.refresh();
  await settle();
  assert.ok(env.root.querySelector('.ob__last').classList.contains('up'));

  await book.refresh(); // та же цена
  await settle();
  assert.ok(
    env.root.querySelector('.ob__last').classList.contains('up'),
    'цвет держится до следующего изменения',
  );
});

test('reset стирает направление: стрелка от чужой цены смысла не имеет', async (t) => {
  const env = setup();
  t.after(env.cleanup);

  let d = depth();
  const { book } = mount(env, { fetch: async () => d });
  await settle();

  d = depth({ bids: [[1000200, 200000]], asks: [[1000400, 150000]] });
  await book.refresh();
  await settle();
  assert.ok(env.root.querySelector('.ob__last').classList.contains('up'));

  book.reset();
  await book.refresh();
  await settle();

  const el = env.root.querySelector('.ob__last');
  assert.ok(!el.classList.contains('up') && !el.classList.contains('down'), 'направления нет');
});

test('одна из сторон пуста — середины нет, и это видно', async (t) => {
  // Пустой стакан не должен выдавать число: «—» отправит разбираться, а
  // придуманная середина — торговать.
  const env = setup();
  t.after(env.cleanup);
  mount(env, { fetch: async () => depth({ asks: [] }) });
  await settle();

  assert.equal(env.root.querySelector('.ob__last').textContent, '—');
  assert.equal(env.root.querySelector('.ob__spread').textContent, '');
});

// ---- Режимы показа ---------------------------------------------------------

test('режим «только покупки» прячет аски и показывает вдвое больше строк', async (t) => {
  const env = setup();
  t.after(env.cleanup);

  const many = (n, from, stepSign) => Array.from({ length: n }, (_, i) => [from + stepSign * i * 100, 100000]);
  const { calls } = mount(env, {
    levels: 3,
    fetch: async () => depth({ bids: many(20, 1000000, -1), asks: many(20, 1000100, 1) }),
  });
  await settle();
  assert.equal(side(env.root, 'bids').length, 3, 'в обычном режиме levels строк');

  click(env.window, env.root.querySelector('.ob__mode[data-m="bids"]'));
  await settle();

  assert.equal(side(env.root, 'asks').length, 0, 'аски убраны');
  assert.equal(side(env.root, 'bids').length, 6, 'одна сторона — вдвое больше строк');
  assert.ok(env.root.querySelector('.asks').classList.contains('off'));
  assert.ok(calls.fetch.length > 1, 'настройка перечитывает стакан, а не ждёт тика');
});

test('глубина запроса растёт вместе с шагом группировки', async (t) => {
  // Грубый шаг съедает уровни пачками: прежняя глубина дала бы две строки
  // вместо двадцати, и стакан выглядел бы пустым.
  const env = setup();
  t.after(env.cleanup);

  env.window.localStorage.setItem('ob.settings', JSON.stringify({ rounding: true, groupMul: 10 }));
  const { calls } = mount(env, { levels: 20 });
  await settle();

  assert.equal(calls.fetch[0][1], 200, 'levels 20 × шаг 10');
});

// ---- Взаимодействие и подписи ---------------------------------------------

test('клик по уровню отдаёт его цену строкой, как она показана', async (t) => {
  const env = setup();
  t.after(env.cleanup);
  const { calls } = mount(env);
  await settle();

  click(env.window, env.root.querySelector('.bids .brow'));

  assert.deepEqual(calls.pick, ['10000.00']);
});

test('подписи колонок берут валюты инструмента', async (t) => {
  const env = setup();
  t.after(env.cleanup);
  mount(env, { symbols: () => ({ base: 'ETH', quote: 'USDT' }) });
  await settle();

  assert.equal(env.root.querySelector('.c-price').textContent, 'Price (USDT)');
  assert.equal(env.root.querySelector('.c-amt').textContent, 'Amount (ETH)');
});

test('инструмент без известных валют не подставляет чужие', async (t) => {
  const env = setup();
  t.after(env.cleanup);
  mount(env, { symbols: () => ({}) });
  await settle();

  assert.equal(env.root.querySelector('.c-price').textContent, 'Price');
  assert.equal(env.root.querySelector('.c-amt').textContent, 'Amount');
});

test('полоса соотношения считает доли объёма и в сумме даёт сто', async (t) => {
  const env = setup();
  t.after(env.cleanup);
  mount(env, {
    fetch: async () => depth({ bids: [[1000000, 300000]], asks: [[1000100, 100000]] }),
  });
  await settle();

  assert.equal(env.root.querySelector('.bv').textContent, '75.00%');
  assert.equal(env.root.querySelector('.sv').textContent, '25.00%');
});

// ---- Отказы ----------------------------------------------------------------

test('отказ источника не стирает прежний стакан', async (t) => {
  const env = setup();
  t.after(env.cleanup);

  let mode = 'ok';
  const { book } = mount(env, {
    fetch: async () => {
      if (mode === 'throw') throw new Error('сеть');
      if (mode === 'junk') return { price_decimals: 2 }; // без asks
      return depth();
    },
  });
  await settle();
  assert.equal(side(env.root, 'bids').length, 3);

  mode = 'throw';
  await book.refresh();
  await settle();
  assert.equal(side(env.root, 'bids').length, 3, 'после исключения стакан на месте');

  mode = 'junk';
  await book.refresh();
  await settle();
  assert.equal(side(env.root, 'bids').length, 3, 'после ответа без уровней тоже');
});

test('инструмент не выбран — за стаканом не ходим', async (t) => {
  const env = setup();
  t.after(env.cleanup);
  const { calls } = mount(env, { instrument: () => null });
  await settle();

  assert.deepEqual(calls.fetch, []);
});

// ---- Настройки -------------------------------------------------------------

test('настройки переживают перемонтирование и не лезут к соседям', async (t) => {
  const env = setup();
  t.after(env.cleanup);

  mount(env, { storagePrefix: 'ob' });
  await settle();
  click(env.window, env.root.querySelector('.ob__mode[data-m="asks"]'));
  await settle();

  // Второй стакан с тем же префиксом видит выбор, с другим — нет.
  const again = env.window.OrderBook.mount(env.root, {
    instrument: () => 1, fetch: async () => depth(), storagePrefix: 'ob', interval: 0,
  });
  await settle();
  assert.ok(env.root.querySelector('.ob__mode[data-m="asks"]').classList.contains('active'));
  again.destroy();

  const other = env.window.OrderBook.mount(env.root, {
    instrument: () => 1, fetch: async () => depth(), storagePrefix: 'ob2', interval: 0,
  });
  await settle();
  assert.ok(
    env.root.querySelector('.ob__mode[data-m="both"]').classList.contains('active'),
    'чужой префикс — свои настройки',
  );
  other.destroy();
});

test('destroy останавливает опрос и очищает стакан', async (t) => {
  const env = setup();
  t.after(env.cleanup);

  let hits = 0;
  const { book } = mount(env, { interval: 5, fetch: async () => { hits += 1; return depth(); } });
  await settle();
  await new Promise((r) => env.window.setTimeout(r, 30));
  const during = hits;
  assert.ok(during > 1, `опрос обязан идти, вызовов ${during}`);

  book.destroy();
  assert.equal(env.root.innerHTML, '');

  await new Promise((r) => env.window.setTimeout(r, 30));
  assert.equal(hits, during, 'после destroy источник не дёргается');
});

// ---- Что сознательно не закреплено ----------------------------------------

test('число знаков объёма в строке стакана сознательно НЕ закреплено', () => {
  // `row()` делит объём на `10 ** qd`, а печатает через `toFixed(3)`. Тот же
  // узор, что в ленте сделок: масштаб берётся у инструмента, разрядность
  // прибита гвоздём. Повторяется в двух виджетах из двух — похоже на принятое
  // решение про ширину колонки, а не на описку, и закреплять его тестом до
  // ответа штаба значило бы объявить решением то, чего никто не решал.
  assert.ok(true);
});
