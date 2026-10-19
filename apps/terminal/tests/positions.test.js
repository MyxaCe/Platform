'use strict';
// ============================================================================
// Нижняя панель терминала: открытые сделки, лимитные ордера, история, сигналы
// (TRM-05, контракт ADR-015 `Widget.mount(root, ctx) → { refresh, destroy }`).
//
// Здесь на экране деньги. Ошибка в этой панели не падает — она рисует неверное
// число, по которому человек принимает решение. Поэтому проверяется не «виджет
// построился», а три вещи по отдельности:
//   1) число на экране собрано из полей САМОЙ позиции (decimals инструмента),
//   2) «нет данных», «нет доступа» и «не смогли» различимы на экране,
//   3) отсутствующий стоп рисуется прочерком, а не нулём.
//
// Третье — тот же класс, что `unwrap_or(0.0)` в потоке котировок: ноль здесь
// правдоподобен, и «стоп на нуле» неотличим от «стопа нет».
// ============================================================================
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadWidget, click } = require('./helpers/load-widget');

/** Позиция в том виде, в каком её отдаёт gateway: целые в минимальных единицах. */
function deal(over) {
  return Object.assign(
    {
      id: 7,
      symbol: 'BTCUSD',
      side: 'buy',
      qty: 1500,
      qty_decimals: 3,
      entry: 6512345,
      mark: 6598765,
      price_decimals: 2,
      pnl: 12345,
    },
    over || {},
  );
}

function order(over) {
  return Object.assign(
    {
      id: 11,
      symbol: 'ETHUSD',
      side: 'sell',
      qty: 2000,
      qty_decimals: 3,
      price: 320000,
      sl: 330000,
      tp: 310000,
      price_decimals: 2,
    },
    over || {},
  );
}

/** Поднять панель с управляемым источником. */
function mount(env, over) {
  const calls = { closeDeal: [], cancelPending: [], onChange: 0, onDeals: [] };
  const api = Object.assign(
    {
      deals: async () => [deal()],
      pending: async () => [],
      closedDeals: async () => [],
      closeDeal: async (id) => { calls.closeDeal.push(id); return { ok: true }; },
      cancelPending: async (id) => { calls.cancelPending.push(id); return { ok: true }; },
    },
    (over && over.api) || {},
  );

  const panel = env.window.Positions.mount(env.root, Object.assign({
    api,
    onChange: () => { calls.onChange += 1; },
    onDeals: (l) => calls.onDeals.push(l),
    signals: () => [],
    interval: 0, // таймер заводим только там, где его и проверяем
  }, over || {}, { api }));

  return { panel, calls };
}

/** Дождаться, пока отработают обещания источника: refresh асинхронный. */
const settle = () => new Promise((r) => setImmediate(r));

const rows = (root, pane) => [...root.querySelectorAll(`[data-rows="${pane}"] .deal-row`)];
const cells = (row) => [...row.children].map((c) => c.textContent);

function setup() {
  const env = loadWidget(['positions.js']);
  return env;
}

// ---- Числа собираются из полей самой позиции -------------------------------

test('строка позиции: объём и цены по decimals самого инструмента', async (t) => {
  const env = setup();
  t.after(env.cleanup);
  mount(env);
  await settle();

  const r = rows(env.root, 'deals');
  assert.equal(r.length, 1);
  assert.deepEqual(cells(r[0]).slice(0, 6), [
    'BTCUSD',
    'BUY',
    '1.500',      // qty 1500 при qty_decimals 3
    '65123.45',   // entry 6512345 при price_decimals 2
    '65987.65',   // mark
    '$123.45',    // pnl 12345 центов
  ]);
});

test('decimals берутся у каждой позиции своей, а не общие на панель', async (t) => {
  // Если бы знаки после запятой брались откуда-то одни на всех, вторая строка
  // показала бы цену, отличающуюся в сто раз, — и выглядела бы правдоподобно.
  const env = setup();
  t.after(env.cleanup);
  mount(env, {
    api: { deals: async () => [deal(), deal({ id: 8, symbol: 'XAUUSD', entry: 203012, price_decimals: 3, qty: 50, qty_decimals: 2 })] },
  });
  await settle();

  const r = rows(env.root, 'deals');
  assert.equal(cells(r[0])[3], '65123.45');
  assert.equal(cells(r[1])[3], '203.012');
  assert.equal(cells(r[1])[2], '0.50');
});

test('знак P&L красит строку, и ноль считается неубыточным', async (t) => {
  const env = setup();
  t.after(env.cleanup);
  mount(env, {
    api: { deals: async () => [deal({ id: 1, pnl: 500 }), deal({ id: 2, pnl: -500 }), deal({ id: 3, pnl: 0 })] },
  });
  await settle();

  const cls = rows(env.root, 'deals').map((r) => r.querySelector('.pnl').className);
  assert.deepEqual(cls, ['pnl pos', 'pnl neg', 'pnl pos']);
  assert.equal(rows(env.root, 'deals')[1].querySelector('.pnl').textContent, '-$5.00');
});

// ---- Отсутствующий стоп — прочерк, а не ноль -------------------------------

test('ордер без SL/TP рисует прочерк, а не нулевую цену', async (t) => {
  // Ноль здесь правдоподобен: «стоп на нуле» и «стопа нет» — разные вещи, и по
  // первой человек решит, что защита стоит. Родня `unwrap_or(0.0)` в котировках.
  const env = setup();
  t.after(env.cleanup);
  mount(env, { api: { pending: async () => [order({ sl: null, tp: null })] } });
  await settle();

  const c = cells(rows(env.root, 'pending')[0]);
  assert.equal(c[3], '3200.00', 'цена ордера на месте');
  assert.equal(c[4], '—', 'SL не задан');
  assert.equal(c[5], '—', 'TP не задан');
});

// ---- Три разных «строк нет» ------------------------------------------------

test('пустой список — это «нет позиций», и так и написано', async (t) => {
  const env = setup();
  t.after(env.cleanup);
  mount(env, { api: { deals: async () => [] } });
  await settle();

  assert.match(env.root.querySelector('[data-rows="deals"]').textContent, /No open positions/);
});

test('нет сессии — пишем причину, а не «нет позиций» (BUG-011)', async (t) => {
  // Отказ не должен выглядеть правдоподобным значением: «No open positions» —
  // это утверждение о счёте, которого мы не читали.
  const env = setup();
  t.after(env.cleanup);
  mount(env, {
    api: { deals: async () => null, tradingStatus: () => 'no-session' },
  });
  await settle();

  const text = env.root.querySelector('[data-rows="deals"]').textContent;
  assert.match(text, /No cabinet session/);
  assert.doesNotMatch(text, /No open positions/);
});

test('сетевой блип не стирает прежние позиции', async (t) => {
  // «Нет позиций» при живых позициях опаснее устаревшего числа: по первому
  // человек решит, что он вне рынка.
  const env = setup();
  t.after(env.cleanup);

  let answer = [deal()];
  const { panel } = mount(env, {
    api: { deals: async () => answer, tradingStatus: () => 'ok' },
  });
  await settle();
  assert.equal(rows(env.root, 'deals').length, 1);

  answer = null; // источник не ответил
  await panel.refresh();
  await settle();

  const r = rows(env.root, 'deals');
  assert.equal(r.length, 1, 'строка осталась');
  assert.equal(cells(r[0])[0], 'BTCUSD');
});

test('позиции не уезжают хосту, когда их не прочитали', async (t) => {
  // Хост рисует по этому списку линии SL/TP на графике. Пустой список при
  // неудачном чтении стёр бы линии у живых позиций.
  const env = setup();
  t.after(env.cleanup);

  let answer = [deal()];
  const { panel, calls } = mount(env, { api: { deals: async () => answer, tradingStatus: () => 'ok' } });
  await settle();
  assert.equal(calls.onDeals.length, 1);

  answer = null;
  await panel.refresh();
  await settle();

  assert.equal(calls.onDeals.length, 1, 'второго вызова быть не должно');
});

// ---- Действия --------------------------------------------------------------

test('Close закрывает именно ту сделку, на которой нажали', async (t) => {
  const env = setup();
  t.after(env.cleanup);
  const { calls } = mount(env, {
    api: { deals: async () => [deal({ id: 7 }), deal({ id: 42, symbol: 'ETHUSD' })] },
  });
  await settle();

  click(env.window, rows(env.root, 'deals')[1].querySelector('.closebtn'));
  await settle();

  assert.deepEqual(calls.closeDeal, [42]);
  assert.equal(calls.onChange, 1, 'метрики счёта обязаны пересчитаться');
});

test('Cancel отменяет ордер, а не закрывает сделку', async (t) => {
  // Пути разные и ручки разные; перепутать их значит закрыть позицию вместо
  // снятия заявки.
  const env = setup();
  t.after(env.cleanup);
  const { calls } = mount(env, { api: { pending: async () => [order({ id: 11 })] } });
  await settle();

  click(env.window, rows(env.root, 'pending')[0].querySelector('.closebtn'));
  await settle();

  assert.deepEqual(calls.cancelPending, [11]);
  assert.deepEqual(calls.closeDeal, []);
});

test('отказ ручки не выдаёт себя за успех', async (t) => {
  const env = setup();
  t.after(env.cleanup);
  const { calls } = mount(env, {
    api: { deals: async () => [deal()], closeDeal: async () => ({ ok: false }) },
  });
  await settle();

  click(env.window, rows(env.root, 'deals')[0].querySelector('.closebtn'));
  await settle();

  assert.equal(calls.onChange, 0, 'метрики не пересчитываем по несостоявшемуся закрытию');
});

// ---- Вкладки ---------------------------------------------------------------

test('открыта ровно одна вкладка, и клик её меняет', async (t) => {
  const env = setup();
  t.after(env.cleanup);
  mount(env);
  await settle();

  const visible = () => [...env.root.querySelectorAll('.pane')]
    .filter((p) => !p.classList.contains('hidden'))
    .map((p) => p.dataset.pane);

  assert.deepEqual(visible(), ['deals']);

  click(env.window, env.root.querySelector('.bottom__tabs [data-pane="pending"]'));
  assert.deepEqual(visible(), ['pending'], 'ровно одна, а не две');
  assert.equal(
    env.root.querySelector('.bottom__tabs .active').dataset.pane,
    'pending',
    'подсветка вкладки идёт за содержимым',
  );
});

// ---- Сигналы ---------------------------------------------------------------

test('сигналов нет — так и написано, а панель не пуста', async (t) => {
  const env = setup();
  t.after(env.cleanup);
  mount(env, { signals: () => [] });
  await settle();

  assert.match(env.root.querySelector('[data-rows="signals"]').textContent, /No signals/);
});

test('сигналы показываются с направлением и его цветом', async (t) => {
  const env = setup();
  t.after(env.cleanup);
  mount(env, { signals: () => [{ symbol: 'BTCUSD', name: 'RSI', tf: '1h', dir: 'SELL' }] });
  await settle();

  const r = rows(env.root, 'signals')[0];
  assert.deepEqual(cells(r), ['BTCUSD', 'RSI', '1h', 'SELL']);
  assert.match(r.querySelector('.dir').className, /sell/);
});

// ---- Уборка ----------------------------------------------------------------

test('destroy останавливает опрос и очищает контейнер', async (t) => {
  const env = setup();
  t.after(env.cleanup);

  let hits = 0;
  const { panel } = mount(env, {
    interval: 5,
    api: { deals: async () => { hits += 1; return [deal()]; } },
  });
  await settle();

  await new Promise((r) => env.window.setTimeout(r, 30));
  const during = hits;
  assert.ok(during > 1, `опрос обязан идти, вызовов ${during}`);

  panel.destroy();
  assert.equal(env.root.innerHTML, '');

  await new Promise((r) => env.window.setTimeout(r, 30));
  assert.equal(hits, during, 'после destroy источник не дёргается');
});

// ---- Что этим набором НЕ проверено ----------------------------------------

test('мутация «панель не смонтирована на экране» здесь непрогоняема', () => {
  // Снятие виджета с экрана не красит ни одного из тестов выше: все они
  // монтируют панель сами, и «никто не монтирует» для них неотличимо от
  // «монтируют». Это класс «написано и не вызывается» на уровне проводки, и
  // ловится он проверкой проводки в `app.js`, которой у нас нет.
  //
  // Записано, чтобы следующий не счёл проводку проверенной: зелёный этот файл
  // остался бы и при пустой нижней панели в живом терминале.
  assert.ok(true);
});
