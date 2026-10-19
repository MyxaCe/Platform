'use strict';
// ============================================================================
// Сделки по инструменту: лента рынка и мои исполнения (TRM-05, ADR-015).
//
// Лента наполняется хостом через `pushMarket` — сокет один на приложение и
// владеет им хост. Отсюда главное свойство виджета: он обязан **отсеивать
// чужое**. Тик по другой паре, попавший в ленту текущей, — это не косметика:
// цены разных инструментов отличаются на порядки, и чужая цена в ленте
// читается как движение рынка, которого не было.
// ============================================================================
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadWidget, click } = require('./helpers/load-widget');

const settle = () => new Promise((r) => setImmediate(r));

function setup() {
  return loadWidget(['trades.js']);
}

function mount(env, over) {
  const ctx = Object.assign(
    {
      instrument: () => 1,
      decimals: () => ({ pd: 2, qd: 5 }),
      myTrades: async () => [],
      interval: 0,
    },
    over || {},
  );
  return env.window.Trades.mount(env.root, ctx);
}

const rows = (root, pane) => [...root.querySelectorAll(`[data-rows="${pane}"] .trow`)];
const texts = (root, pane) => rows(root, pane).map((r) => [...r.children].map((c) => c.textContent));

function myDeal(over) {
  return Object.assign(
    {
      side: 'buy',
      qty: 1500,
      qty_decimals: 3,
      entry: 6500000,
      exit: 6600000,
      price_decimals: 2,
      pnl: 15000,
    },
    over || {},
  );
}

// ---- Лента рынка -----------------------------------------------------------

test('пустая лента объясняет себя, а не молчит', (t) => {
  const env = setup();
  t.after(env.cleanup);
  mount(env);

  assert.match(env.root.querySelector('[data-rows="market"]').textContent, /Waiting for trades/);
});

test('тик по другой паре в ленту не попадает', (t) => {
  // Цены разных инструментов отличаются на порядки: чужая цена в ленте
  // читается как движение, которого не было.
  const env = setup();
  t.after(env.cleanup);
  const w = mount(env, { instrument: () => 1 });

  w.pushMarket({ instrument: 2, price: 100, qty: 1, side: 'buy' });

  assert.equal(rows(env.root, 'market').length, 0);
  assert.match(env.root.querySelector('[data-rows="market"]').textContent, /Waiting for trades/);
});

test('инструмент не выбран — лента не принимает ничего', (t) => {
  const env = setup();
  t.after(env.cleanup);
  const w = mount(env, { instrument: () => null });

  w.pushMarket({ instrument: null, price: 100, qty: 1, side: 'buy' });

  assert.equal(rows(env.root, 'market').length, 0);
});

test('свежий тик встаёт наверх, а не в конец', (t) => {
  // Лента читается сверху: порядок здесь — это «что сейчас», а не оформление.
  const env = setup();
  t.after(env.cleanup);
  const w = mount(env);

  w.pushMarket({ instrument: 1, price: 100000, qty: 100000, side: 'buy' });
  w.pushMarket({ instrument: 1, price: 200000, qty: 100000, side: 'sell' });

  assert.deepEqual(texts(env.root, 'market').map((r) => r[0]), ['2000.00', '1000.00']);
});

test('лента не растёт бесконечно и сохраняет самые свежие', (t) => {
  const env = setup();
  t.after(env.cleanup);
  const w = mount(env, { max: 3 });

  for (let i = 1; i <= 5; i += 1) {
    w.pushMarket({ instrument: 1, price: i * 100000, qty: 100000, side: 'buy' });
  }

  const prices = texts(env.root, 'market').map((r) => r[0]);
  assert.equal(prices.length, 3, 'длина ограничена');
  assert.deepEqual(prices, ['5000.00', '4000.00', '3000.00'], 'выброшены старые, а не новые');
});

test('сторона сделки красит цену, и неизвестная сторона не красит в продажу', (t) => {
  const env = setup();
  t.after(env.cleanup);
  const w = mount(env);

  w.pushMarket({ instrument: 1, price: 100000, qty: 1, side: 'sell' });
  w.pushMarket({ instrument: 1, price: 100000, qty: 1, side: 'buy' });
  w.pushMarket({ instrument: 1, price: 100000, qty: 1, side: undefined });

  const cls = rows(env.root, 'market').map((r) => r.querySelector('.tp').className);
  assert.deepEqual(cls, ['tp buy', 'tp buy', 'tp sell']);
});

test('цена ленты считается по знакам текущего инструмента', (t) => {
  const env = setup();
  t.after(env.cleanup);
  const w = mount(env, { decimals: () => ({ pd: 3, qd: 5 }) });

  w.pushMarket({ instrument: 1, price: 203012, qty: 100000, side: 'buy' });

  assert.equal(texts(env.root, 'market')[0][0], '203.012');
});

test('смена инструмента стирает чужую ленту', (t) => {
  // Старые тики к новой паре отношения не имеют, а выглядели бы как её цены.
  const env = setup();
  t.after(env.cleanup);
  const w = mount(env);

  w.pushMarket({ instrument: 1, price: 100000, qty: 1, side: 'buy' });
  assert.equal(rows(env.root, 'market').length, 1);

  w.clearTape();

  assert.equal(rows(env.root, 'market').length, 0);
  assert.match(env.root.querySelector('[data-rows="market"]').textContent, /Waiting for trades/);
});

// ---- Мои сделки ------------------------------------------------------------

test('моя сделка показывает объём, вход → выход и P&L', async (t) => {
  const env = setup();
  t.after(env.cleanup);
  mount(env, { myTrades: async () => [myDeal()], fmtUsd: (c) => `$${(c / 100).toFixed(2)}` });
  await settle();

  assert.deepEqual(texts(env.root, 'mine')[0], ['BUY 1.500', '65000.00 → 66000.00', '$150.00']);
});

test('знак P&L красит строку, и ноль не считается убытком', async (t) => {
  const env = setup();
  t.after(env.cleanup);
  mount(env, { myTrades: async () => [myDeal({ pnl: -1 }), myDeal({ pnl: 0 })] });
  await settle();

  const cls = rows(env.root, 'mine').map((r) => r.querySelector('.pnl').className);
  assert.deepEqual(cls, ['pnl neg', 'pnl pos']);
});

test('сделок по инструменту нет — так и написано', async (t) => {
  const env = setup();
  t.after(env.cleanup);
  mount(env, { myTrades: async () => [] });
  await settle();

  assert.match(env.root.querySelector('[data-rows="mine"]').textContent, /No trades on this instrument/);
});

test('мои сделки запрашиваются для текущего инструмента', async (t) => {
  const env = setup();
  t.after(env.cleanup);

  const asked = [];
  let current = 1;
  const w = mount(env, { instrument: () => current, myTrades: async (id) => { asked.push(id); return []; } });
  await settle();

  current = 42;
  await w.refreshMine();
  await settle();

  assert.deepEqual(asked, [1, 42]);
});

test('инструмент не выбран — за моими сделками не ходим', async (t) => {
  const env = setup();
  t.after(env.cleanup);

  const asked = [];
  mount(env, { instrument: () => null, myTrades: async (id) => { asked.push(id); return []; } });
  await settle();

  assert.deepEqual(asked, []);
});

// ---- Вкладки и уборка ------------------------------------------------------

test('открыта ровно одна вкладка, и клик её меняет', (t) => {
  const env = setup();
  t.after(env.cleanup);
  mount(env);

  const visible = () => [...env.root.querySelectorAll('.trades__pane')]
    .filter((p) => !p.classList.contains('hidden'))
    .map((p) => p.dataset.t);

  assert.deepEqual(visible(), ['market']);

  click(env.window, env.root.querySelector('.trades__tabs [data-t="mine"]'));

  assert.deepEqual(visible(), ['mine'], 'ровно одна, а не две');
  assert.equal(env.root.querySelector('.trades__tabs button.active').dataset.t, 'mine');
});

test('destroy останавливает опрос и очищает виджет', async (t) => {
  const env = setup();
  t.after(env.cleanup);

  let hits = 0;
  const w = mount(env, { interval: 5, myTrades: async () => { hits += 1; return []; } });
  await settle();
  await new Promise((r) => env.window.setTimeout(r, 30));
  const during = hits;
  assert.ok(during > 1, `опрос обязан идти, вызовов ${during}`);

  w.destroy();
  assert.equal(env.root.innerHTML, '');

  await new Promise((r) => env.window.setTimeout(r, 30));
  assert.equal(hits, during, 'после destroy источник не дёргается');
});

// ---- Разрядность объёма (Р-064) -------------------------------------------

test('объём в ленте показывается со знаками своего инструмента', (t) => {
  // Было `toFixed(3)` при делении на `10 ** qd`: масштаб брался у инструмента,
  // точность — нет. Штаб признал это опиской (Р-064): повторение в двух
  // виджетах из двух говорит о копировании, а решение про ширину колонки
  // выглядело бы как решение про ширину, а не как потеря значащих цифр.
  const env = setup();
  t.after(env.cleanup);
  const w = mount(env, { decimals: () => ({ pd: 2, qd: 5 }) });

  w.pushMarket({ instrument: 1, price: 100000, qty: 120, side: 'buy' });

  assert.equal(texts(env.root, 'market')[0][1], '0.00120');
});

test('два разных объёма не выглядят одним числом', (t) => {
  // Цена ошибки — клиента: он видит на торговом экране одно и то же там, где
  // объёмы отличаются втрое.
  const env = setup();
  t.after(env.cleanup);
  const w = mount(env, { decimals: () => ({ pd: 2, qd: 5 }) });

  w.pushMarket({ instrument: 1, price: 100000, qty: 60, side: 'buy' });
  w.pushMarket({ instrument: 1, price: 100000, qty: 140, side: 'buy' });

  const shown = texts(env.root, 'market').map((r) => r[1]);
  assert.equal(new Set(shown).size, 2, `разные объёмы показаны одинаково: ${shown.join(' / ')}`);
});

test('инструмент с грубым шагом объёма не получает лишних нулей', (t) => {
  const env = setup();
  t.after(env.cleanup);
  const w = mount(env, { decimals: () => ({ pd: 2, qd: 2 }) });

  w.pushMarket({ instrument: 1, price: 100000, qty: 150, side: 'buy' });

  assert.equal(texts(env.root, 'market')[0][1], '1.50');
});
