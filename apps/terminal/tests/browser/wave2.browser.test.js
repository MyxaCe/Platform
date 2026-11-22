'use strict';
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');
const assert = require('node:assert/strict');
const { openTerminal, wave2Routes } = require('../helpers/browser');

const settle = (page, ms) => page.evaluate((d) => new Promise((r) => setTimeout(r, d)), ms || 600);

const ACCOUNT = {
  '/account': () => ({
    opened: true, balance: 1000000, equity: 1000000, used_margin: 0, free_margin: 1000000, open_pnl: 0,
    model: 'hedging', leverage: 10, kind: 'demo', min_stop_bps: 0,
  }),
};

/**
 * @param {any} t
 * @param {{ tag?: string, over?: any, routes?: Record<string, Function>, lang?: string }} [opts]
 */
async function open(t, opts) {
  const o = opts || {};
  const term = await openTerminal({
    query: `?site=apex-ru#handoff=h-w2${o.tag || ''}&account=demo&lang=${o.lang || 'ru'}`,
    routes: Object.assign(wave2Routes(o.over), ACCOUNT, o.routes || {}),
  });
  t.after(term.cleanup);
  await settle(term.page, 1800);
  return term;
}

/**
 * @param {any} term
 * @param {number} id
 */
async function pick(term, id) {
  await term.page.click(`.irow[data-id="${id}"]`);
  await settle(term.page, 1200);
}

/** @param {any} page */
const ui = (page) => page.evaluate(() => {
  const $ = (/** @type {string} */ id) => /** @type {HTMLElement} */ (document.getElementById(id));
  const shown = (/** @type {Element|null} */ el) => !!el && el.getBoundingClientRect().height > 0;
  return {
    market: $('ihMarket').hidden ? null : $('ihMarket').textContent,
    gate: $('dealGate').hidden ? null : $('dealGate').textContent,
    buyOff: /** @type {HTMLButtonElement} */ ($('btnBuy')).disabled,
    sellOff: /** @type {HTMLButtonElement} */ ($('btnSell')).disabled,
    buyTip: $('btnBuy').title,
    oneSec: /** @type {HTMLButtonElement} */ (document.querySelector('#tfs button[data-tf="1"]')).disabled,
    activeTf: /** @type {HTMLElement} */ (document.querySelector('#tfs button.active')).dataset.tf,
    bookOff: $('book').classList.contains('ob--off'),
    bookText: shown(document.querySelector('#book .ob__off')) ? document.querySelector('#book .ob__off').textContent : null,
    bookRows: document.querySelectorAll('#book .brow').length,
    tape: document.querySelector('#trades [data-rows="market"]').textContent,
    sizeHint: $('sizeHint').hidden ? null : $('sizeHint').textContent,
    sizeEq: $('sizeEq').textContent,
    lot: /** @type {HTMLInputElement} */ ($('lot')).value,
    pair: $('ihBase').textContent + $('ihQuote').textContent,
    last: $('ihLast').textContent,
    msg: $('dealMsg').textContent,
  };
});

/**
 * @param {any} page
 * @param {number} id
 */
const badge = (page, id) => page.evaluate((i) => {
  const el = document.querySelector(`.irow[data-id="${i}"] .wl-mk`);
  return el ? { text: el.textContent, tip: /** @type {HTMLElement} */ (el).title } : null;
}, id);

/**
 * @param {any[]} seen
 * @param {string} p
 * @param {string} [method]
 */
const hits = (seen, p, method) => seen.filter((r) => r.path === p && (!method || r.method === method));

test('волна 2: терминал открывается, список из CMS с пометками рынка, крипто как раньше', async (t) => {
  const term = await open(t);
  assert.deepEqual(term.errors, [], 'страница обязана молчать в консоль');
  const rows = await term.page.evaluate(() => [...document.querySelectorAll('.irow')].map((r) => /** @type {HTMLElement} */ (r).dataset.id));
  assert.deepEqual(rows, ['1', '6', '3', '4', '5'], '«Все»: блоками по категориям, внутри — порядок CMS');
  assert.equal(await badge(term.page, 1), null);
  const eur = await badge(term.page, 3);
  assert.equal(eur.text, 'Закрыт');
  assert.match(eur.tip, /^Рынок закрыт · откроется /);
  const ltc = await badge(term.page, 6);
  assert.equal(ltc.text, 'Закрытие');
  assert.match(ltc.tip, /^Только закрытие: /);
  assert.equal(await badge(term.page, 4), null);

  const btc = await ui(term.page);
  assert.equal(btc.pair, 'BTC/USDT');
  assert.notEqual(await term.page.evaluate(() => getComputedStyle(document.querySelector('.ih-source')).display), 'none', 'у крипто источник назван, как раньше');
  assert.equal(btc.market, null);
  assert.equal(btc.gate, null);
  assert.equal(btc.buyOff, false);
  assert.equal(btc.oneSec, false);
  assert.equal(btc.bookOff, false);
  assert.ok(btc.bookRows > 0, 'стакан крипто на месте');
  assert.equal(btc.sizeHint, null, 'тривиальный минимум не шумит');
  assert.match(btc.sizeEq, /≈/);

  await term.page.click('#btnBuy');
  await settle(term.page, 500);
  const sent = hits(term.seen, '/deals', 'POST').map((r) => JSON.parse(r.body));
  assert.equal(sent.length, 1);
  assert.equal(sent[0].qty, 1000, '0.01 BTC при qty_decimals 5 — как раньше');
  for (const id of [3, 4, 5]) assert.equal(hits(term.seen, `/depth/${id}`).length, 0, `стакан ${id} не запрашивается`);
});

test('EURUSD при закрытом рынке: пометка со временем из сессии, рынок закрыт, отложенный ордер в лотах', async (t) => {
  const tfs = [];
  const routes = wave2Routes({ nextOpen: null });
  const base = routes['/candles/3'];
  const term = await open(t, {
    tag: '-eur',
    over: { nextOpen: null },
    routes: { '/candles/3': (/** @type {URL} */ u) => { tfs.push(u.searchParams.get('tf')); return base(u); } },
  });
  await term.page.click('#tfs button[data-tf="1"]');
  await settle(term.page, 600);
  await pick(term, 3);

  const s = await ui(term.page);
  assert.equal(s.pair, 'EUR/USD');
  assert.equal(await term.page.evaluate(() => getComputedStyle(document.querySelector('.ih-source')).display), 'none', 'не крипто — подпись «Цены: Binance» не врёт');
  assert.match(s.market, /^Рынок закрыт · откроется \S/, `пометка: «${s.market}»`);
  assert.match(s.gate, /Рыночные ордера недоступны до открытия/);
  assert.ok(s.buyOff && s.sellOff, 'рыночная сделка неактивна');
  assert.match(s.buyTip, /Рынок закрыт/);
  assert.equal(s.oneSec, true, '1s недоступен');
  assert.equal(s.activeTf, '60', 'секундный ТФ сменился на минутный');
  assert.ok(!tfs.includes('1'), `секундные свечи не запрашивались: ${tfs}`);
  assert.ok(s.bookOff);
  assert.match(s.bookText, /Стакана по этому инструменту нет/);
  assert.equal(hits(term.seen, '/depth/3').length, 0);
  assert.match(s.tape, /Ленты сделок по этому инструменту нет/);
  assert.equal(s.sizeHint, 'Мин. 0,01 лот · шаг 0,01 лот');
  assert.match(s.sizeEq, /\$1\s?084,14|1\s?084,14/, `стоимость 0.01 лота: «${s.sizeEq}»`);

  await term.page.click('#tabLimit');
  await settle(term.page, 300);
  const lim = await ui(term.page);
  assert.equal(lim.buyOff, false, 'отложенный ордер при закрытом рынке разрешён');
  assert.match(lim.gate, /ордер дождётся открытия/);
  const lot = /** @type {any} */ (await term.page.$('#lot'));
  await lot.click({ clickCount: 3 });
  await lot.type('0.257');
  await settle(term.page, 200);
  assert.match((await ui(term.page)).sizeHint, /Уйдёт как 0,25 лот/);
  await term.page.click('#btnBuy');
  await settle(term.page, 500);
  const sent = hits(term.seen, '/pending', 'POST').map((r) => JSON.parse(r.body));
  assert.equal(sent.length, 1);
  assert.equal(sent[0].instrument, 3);
  assert.equal(sent[0].qty, 25000, '0.25 лота = 25 000 единиц, округлено вниз до шага');
});

test('XAUUSD: живая свеча и цена по quote, смена статуса из потока применяется сразу', async (t) => {
  const term = await open(t, { tag: '-xau' });
  await pick(term, 4);
  const before = await ui(term.page);
  assert.equal(before.market, null);
  assert.equal(before.buyOff, false);
  assert.equal(before.bookOff, true, 'у металла стакана нет');

  await term.page.setRequestInterception(true);
  term.page.on('request', (/** @type {any} */ req) => {
    if (req.url().includes('/instruments')) req.abort('failed').catch(() => {});
    else req.continue().catch(() => {});
  });
  await settle(term.page, 1200);

  term.push([{ type: 'quote', instrument: 4, bid: 265500, ask: 265524, price: 265512, time: Date.now() }]);
  await settle(term.page, 500);
  const live = await ui(term.page);
  assert.match(live.last, /2\s655,12/, `последняя цена по mid: «${live.last}»`);
  const legend = await term.page.evaluate(() => document.getElementById('lgOhlc').textContent);
  assert.match(legend, /C\s+2\s655,12/, `свеча закрылась по mid: «${legend}»`);
  assert.match(live.tape, /Ленты сделок/, 'quote в ленту сделок не попадает');

  term.push([{ type: 'market', instrument: 4, status: 'closed', next_open: Date.now() + 5 * 3600 * 1000, reason: 'daily break' }]);
  await settle(term.page, 200);
  const closed = await ui(term.page);
  assert.match(closed.market, /^Рынок закрыт · откроется/);
  assert.ok(closed.buyOff);
  assert.equal((await badge(term.page, 4)).text, 'Закрыт');

  term.push([{ type: 'market', instrument: 4, status: 'halted', next_open: null, reason: 'provider' }]);
  await settle(term.page, 200);
  const halted = await ui(term.page);
  assert.equal(halted.market, 'Торги приостановлены');
  assert.match(halted.gate, /пока торги приостановлены/);

  term.push([{ type: 'market', instrument: 4, status: 'open', next_open: null, reason: null }]);
  await settle(term.page, 200);
  const reopened = await ui(term.page);
  assert.equal(reopened.market, null);
  assert.equal(reopened.buyOff, false);
});

test('отказ шлюза «рынок закрыт» показан словами и со временем открытия', async (t) => {
  const at = Date.now() + 26 * 3600 * 1000;
  const term = await open(t, {
    tag: '-rej',
    routes: { '/deals': (/** @type {any} */ _u, /** @type {any} */ req) => (req.method === 'POST' ? { __status: 409, body: { error: 'market_closed', next_open: at } } : []) },
  });
  await pick(term, 4);
  await term.page.click('#btnBuy');
  await settle(term.page, 500);
  const sent = hits(term.seen, '/deals', 'POST').map((r) => JSON.parse(r.body));
  assert.equal(sent[0].qty, 100, '0.01 лота золота = 1 унция = 100 единиц');
  const s = await ui(term.page);
  assert.match(s.msg, /Рынок по этому инструменту закрыт/);
  assert.match(s.msg, /Откроется \S/);
  assert.doesNotMatch(s.msg, /market_closed/);
});

test('AAPL: стакана нет и он не запрашивается, лента сделок есть, объём ниже минимума не отправить', async (t) => {
  const term = await open(t, { tag: '-aapl' });
  await pick(term, 5);
  const s = await ui(term.page);
  assert.equal(s.pair, 'AAPL/USD');
  assert.equal(s.bookOff, true);
  assert.equal(hits(term.seen, '/depth/5').length, 0);
  assert.match(s.tape, /Ждём сделки/);
  assert.equal(s.lot, '0.01', 'введённое человеком не переписывается молча');
  assert.equal(s.buyOff, true, 'ниже минимума кнопка неактивна');
  assert.match(s.gate, /Объём меньше минимального 1 AAPL/);
  await term.page.click('#lotPlus');
  await settle(term.page, 200);
  const up = await ui(term.page);
  assert.equal(up.lot, '1');
  assert.equal(up.buyOff, false);
  assert.equal(up.gate, null);
  term.push([{ type: 'trade', instrument: 5, price: 22820, qty: 3, taker_side: 'buy', time: Date.now() }]);
  await settle(term.page, 300);
  const rows = await term.page.evaluate(() => document.querySelectorAll('#trades [data-rows="market"] .trow').length);
  assert.equal(rows, 1);
});

test('closeOnly: открыть и поставить ордер нельзя, закрыть можно; при закрытом рынке закрытие неактивно', async (t) => {
  const deals = [
    { id: 21, instrument: 6, symbol: 'LTC-USDT', side: 'buy', qty: 1500, qty_decimals: 3, entry: 8000, mark: 8215, price_decimals: 2, pnl: 322, sl: null, tp: null },
    { id: 22, instrument: 3, symbol: 'EURUSD', side: 'sell', qty: 25000, qty_decimals: 0, entry: 108600, mark: 108415, price_decimals: 5, pnl: 4625, sl: null, tp: null },
  ];
  const term = await open(t, {
    tag: '-ltc',
    routes: {
      '/deals': () => deals,
      '/deals/21/close': () => ({ id: 21, exit: 8214 }),
    },
  });
  await pick(term, 6);
  const s = await ui(term.page);
  assert.equal(s.market, 'Только закрытие');
  assert.ok(s.buyOff && s.sellOff);
  assert.match(s.gate, /Только закрытие: новые позиции и ордера не принимаются/);
  await term.page.click('#tabLimit');
  await settle(term.page, 300);
  assert.equal((await ui(term.page)).buyOff, true, 'отложенный ордер в closeOnly тоже нельзя');

  const rows = await term.page.evaluate(() => [...document.querySelectorAll('[data-rows="deals"] .deal-row')].map((r) => {
    const b = /** @type {HTMLButtonElement} */ (r.querySelector('.closebtn'));
    return { sym: r.querySelector('.c-sym').textContent, qty: r.querySelector('.c-qty').textContent, off: b.disabled, tip: b.title };
  }));
  const ltc = rows.find((r) => r.sym === 'LTC-USDT');
  const eur = rows.find((r) => r.sym === 'EURUSD');
  assert.equal(ltc.off, false, 'closeOnly закрывать разрешает');
  assert.equal(ltc.qty, '1,500');
  assert.equal(eur.off, true, 'закрыть по устаревшей цене при закрытом рынке нельзя');
  assert.match(eur.tip, /Рынок закрыт — позицию можно будет закрыть после открытия/);
  assert.equal(eur.qty, '0,25 лот');

  await term.page.click('[data-rows="deals"] .deal-row:first-child .closebtn');
  await settle(term.page, 400);
  assert.equal(hits(term.seen, '/deals/21/close', 'POST').length, 1);
});

test('скрины волны 2 (только при TERMINAL_SHOTS_DIR)', { skip: !process.env.TERMINAL_SHOTS_DIR }, async (t) => {
  const dir = /** @type {string} */ (process.env.TERMINAL_SHOTS_DIR);
  fs.mkdirSync(dir, { recursive: true });
  const deals = [
    { id: 22, instrument: 3, symbol: 'EURUSD', side: 'sell', qty: 25000, qty_decimals: 0, entry: 108600, mark: 108415, price_decimals: 5, pnl: 4625, sl: null, tp: null },
    { id: 21, instrument: 6, symbol: 'LTC-USDT', side: 'buy', qty: 1500, qty_decimals: 3, entry: 8000, mark: 8215, price_decimals: 2, pnl: 322, sl: null, tp: null },
  ];
  const term = await open(t, { tag: '-shots', routes: { '/deals': () => deals } });
  await term.page.setViewport({ width: 1600, height: 960 });
  /** @param {string} v */
  const size = async (v) => {
    const lot = /** @type {any} */ (await term.page.$('#lot'));
    await lot.click({ clickCount: 3 });
    await lot.type(v);
  };
  /** @type {Array<[number, string, string]>} */
  const shots = [[3, 'eurusd-closed', '0.10'], [4, 'xauusd', '0.05'], [5, 'aapl-no-depth', '10'], [6, 'ltc-close-only', '1.5']];
  for (const [id, name, v] of shots) {
    await pick(term, id);
    await size(v);
    await settle(term.page, 800);
    fs.writeFileSync(path.join(dir, `${name}.png`), await term.page.screenshot());
  }
  await term.page.click('#tabLimit');
  await pick(term, 3);
  await size('0.257');
  await settle(term.page, 600);
  fs.writeFileSync(path.join(dir, 'eurusd-closed-limit.png'), await term.page.screenshot());
});

/**
 * @param {any} t
 * @param {string} query
 */
async function openWith(t, query) {
  const term = await openTerminal({ query });
  t.after(term.cleanup);
  await settle(term.page, 1500);
  return term;
}
const chosen = (/** @type {any} */ page) => page.evaluate(() => ({
  row: /** @type {HTMLElement|null} */ (document.querySelector('.irow.active'))?.dataset.id,
  pair: document.getElementById('ihBase').textContent,
  warn: !(/** @type {HTMLElement} */ (document.getElementById('dataWarn')).hidden),
}));

test('?symbol=ETHUSD открывает ETH, опрос списка не перевыбирает', async (t) => {
  const term = await openWith(t, '?site=apex-ru&lang=ru&symbol=ETHUSD');
  assert.deepEqual(await chosen(term.page), { row: '2', pair: 'ETH', warn: false });
  await term.page.click('.irow[data-id="1"]');
  await settle(term.page, 2500);
  assert.equal((await chosen(term.page)).row, '1', 'выбор человека главнее параметра');
  assert.deepEqual(term.errors, []);
});

test('?symbol=NOPE — инструмент по умолчанию, без ошибки и баннера', async (t) => {
  const term = await openWith(t, '?site=apex-ru&lang=ru&symbol=NOPE');
  assert.deepEqual(await chosen(term.page), { row: '1', pair: 'BTC', warn: false });
  assert.deepEqual(term.errors, []);
});

test('?symbol переживает вход по handoff: фрагмент стёрт, параметр на месте', async (t) => {
  const term = await openWith(t, '?site=apex-ru&lang=ru&symbol=ETHUSD#handoff=h-sym&account=demo');
  const loc = await term.page.evaluate(() => ({ search: location.search, hash: location.hash }));
  assert.match(loc.search, /symbol=ETHUSD/);
  assert.equal(loc.hash, '');
  assert.equal((await chosen(term.page)).row, '2');
});
