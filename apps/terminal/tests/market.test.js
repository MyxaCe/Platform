'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadModule } = require('./helpers/load-module');

const Market = loadModule('market.js', 'Market');
const plain = (v) => JSON.parse(JSON.stringify(v));

const FOREX = { timezone: 'America/New_York', weekly: [{ open: 'sun 17:00', close: 'fri 17:00' }], holidays: ['2026-12-25'] };
const NYSE = {
  timezone: 'America/New_York',
  weekly: ['mon', 'tue', 'wed', 'thu', 'fri'].map((d) => ({ open: `${d} 09:30`, close: `${d} 16:00` })),
  holidays: ['2026-11-26'],
};
const SAT_NOON_UTC = Date.UTC(2026, 10, 21, 12, 0);

test('без поля market инструмент открыт и торгуется — старый ответ шлюза ведёт себя как раньше', () => {
  const crypto = { id: 1, symbol: 'BTC-USDT' };
  assert.deepEqual(plain(Market.state(crypto)), { status: 'open', next_open: null, reason: null, tradable: true });
  assert.equal(Market.blockOpen(crypto), null);
  assert.equal(Market.blockPending(crypto), null);
  assert.equal(Market.blockClose(crypto), null);
  assert.equal(Market.nextOpen(crypto, SAT_NOON_UTC), null);
});

test('закрытый рынок: рынок и закрытие запрещены, отложенный ордер разрешён', () => {
  const it = { market: { status: 'closed', next_open: 1795284000000, reason: 'weekend', tradable: false } };
  assert.equal(Market.blockOpen(it), 'closed');
  assert.equal(Market.blockPending(it), null);
  assert.equal(Market.blockClose(it), 'closed');
  assert.equal(Market.nextOpen(it, SAT_NOON_UTC), 1795284000000);
});

test('приостановка: рынок и закрытие запрещены, время открытия не обещаем', () => {
  const it = { market: { status: 'halted', next_open: 1795284000000, reason: 'provider', tradable: false } };
  assert.equal(Market.blockOpen(it), 'halted');
  assert.equal(Market.blockPending(it), null);
  assert.equal(Market.blockClose(it), 'halted');
  assert.equal(Market.nextOpen(it, SAT_NOON_UTC), null);
});

test('closeOnly: открыть и поставить ордер нельзя, закрыть можно', () => {
  const it = { mode: 'closeOnly', market: { status: 'open', next_open: null, reason: null, tradable: true } };
  assert.equal(Market.blockOpen(it), 'closeOnly');
  assert.equal(Market.blockPending(it), 'closeOnly');
  assert.equal(Market.blockClose(it), null);
  assert.equal(Market.blockOpen({ mode: 'trade' }), null);
});

test('открытый, но не торгуемый рынок: открытие запрещено, закрытие — нет', () => {
  const it = { market: { status: 'open', next_open: null, reason: null, tradable: false } };
  assert.equal(Market.blockOpen(it), 'untradable');
  assert.equal(Market.blockClose(it), null);
});

test('незнакомый статус не считается открытым рынком', () => {
  const it = { market: { status: 'maintenance', tradable: true } };
  assert.equal(Market.state(it).status, 'halted');
  assert.equal(Market.state(it).tradable, false);
  assert.equal(Market.blockOpen(it), 'halted');
});

test('без next_open время открытия берётся из расписания сессии', () => {
  const it = { market: { status: 'closed', next_open: null, reason: 'weekend', tradable: false }, session: FOREX };
  assert.equal(Market.nextOpen(it, SAT_NOON_UTC), Date.UTC(2026, 10, 22, 22, 0));
});

test('расписание: ближайшее открытие внутри недели, праздник пропускается', () => {
  const wedEvening = Date.UTC(2026, 10, 25, 23, 0);
  assert.equal(Market.fromSession(NYSE, wedEvening), Date.UTC(2026, 10, 27, 14, 30), 'четверг — праздник, открытие в пятницу');
  const monMorning = Date.UTC(2026, 10, 23, 12, 0);
  assert.equal(Market.fromSession(NYSE, monMorning), Date.UTC(2026, 10, 23, 14, 30), 'сегодня, ещё не открылся');
});

test('расписание учитывает переход на летнее время пояса биржи', () => {
  const satBeforeDst = Date.UTC(2027, 2, 13, 12, 0);
  assert.equal(Market.fromSession(FOREX, satBeforeDst), Date.UTC(2027, 2, 14, 21, 0));
});

test('расписание без недели, с мусором или чужим поясом — времени нет', () => {
  assert.equal(Market.fromSession(null, SAT_NOON_UTC), null);
  assert.equal(Market.fromSession({ timezone: 'UTC', weekly: [] }, SAT_NOON_UTC), null);
  assert.equal(Market.fromSession({ timezone: 'UTC', weekly: [{ open: 'someday' }] }, SAT_NOON_UTC), null);
  assert.equal(Market.fromSession({ timezone: 'Mars/Olympus', weekly: [{ open: 'sun 17:00' }] }, SAT_NOON_UTC), null);
});

test('«откроется …»: сегодня — только время, другой день — с днём недели и датой', () => {
  const now = Date.UTC(2026, 10, 22, 12, 0);
  assert.equal(Market.when(Date.UTC(2026, 10, 22, 22, 0), 'en-US', now, 'UTC'), '22:00');
  const later = Market.when(Date.UTC(2026, 10, 23, 14, 30), 'en-US', now, 'UTC');
  assert.match(later, /Mon/);
  assert.match(later, /23/);
  assert.match(later, /14:30/);
  assert.match(Market.when(Date.UTC(2026, 10, 23, 14, 30), 'ru-RU', now, 'UTC'), /пн.*23.*14:30/i);
});

test('сообщение quote: середина из price, иначе из bid/ask; битое отбрасывается', () => {
  assert.deepEqual(plain(Market.quoteOf({ type: 'quote', instrument: 7, bid: 108412, ask: 108415, price: 108414, time: 1795086260017 })),
    { instrument: 7, bid: 108412, ask: 108415, price: 108414, time: 1795086260017 });
  assert.equal(Market.quoteOf({ type: 'quote', instrument: 7, bid: 100, ask: 103 }).price, 102);
  assert.equal(Market.quoteOf({ type: 'quote', instrument: 7, bid: 100, ask: 103 }).time, null);
  assert.equal(Market.quoteOf({ type: 'quote', instrument: 7, bid: null, ask: null }), null);
  assert.equal(Market.quoteOf({ type: 'quote', instrument: '7', price: 100 }), null);
  assert.equal(Market.quoteOf({ type: 'trade', instrument: 7, price: 100 }), null);
  assert.equal(Market.quoteOf(null), null);
});

test('сообщение market: статус применяется, торгуемость следует из статуса', () => {
  assert.deepEqual(plain(Market.statusOf({ type: 'market', instrument: 3, status: 'closed', next_open: 1795284000000, reason: 'weekend' })),
    { instrument: 3, market: { status: 'closed', next_open: 1795284000000, reason: 'weekend', tradable: false } });
  assert.equal(Market.statusOf({ type: 'market', instrument: 3, status: 'open', next_open: null, reason: null }).market.tradable, true);
  assert.equal(Market.statusOf({ type: 'market', instrument: 3, status: 'weird' }), null);
  assert.equal(Market.statusOf({ type: 'market', status: 'open' }), null);
});

test('символ платформы из адреса: нормализация как в шлюзе', () => {
  assert.equal(Market.normSymbol(' btc-usdt '), 'BTCUSDT');
  assert.equal(Market.normSymbol('eur/usd'), 'EURUSD');
  assert.equal(Market.normSymbol('BRK.B_x'), 'BRKBX');
  assert.equal(Market.normSymbol(null), '');
});

test('символ платформы находит инструмент по canonical, symbol и base+USD у USDT', () => {
  const list = [
    { id: 1, symbol: 'BTC-USDT' },
    { id: 2, symbol: 'ETH-USDT', canonical: 'ETHUSD' },
    { id: 3, symbol: 'EURUSD', canonical: 'EURUSD' },
    { id: 4, symbol: 'XAU/USD' },
  ];
  assert.equal(Market.matchSymbol(list, 'BTCUSD'), 1, 'BTCUSDT ≈ BTCUSD');
  assert.equal(Market.matchSymbol(list, 'btc-usdt'), 1);
  assert.equal(Market.matchSymbol(list, 'ethusd'), 2);
  assert.equal(Market.matchSymbol(list, 'EUR-USD'), 3);
  assert.equal(Market.matchSymbol(list, 'XAUUSD'), 4);
  assert.equal(Market.matchSymbol(list, 'NOPE'), null);
  assert.equal(Market.matchSymbol(list, ''), null);
  assert.equal(Market.matchSymbol(null, 'BTCUSD'), null);
  assert.equal(Market.matchSymbol([{ id: 9, symbol: 'BTCUSD' }, { id: 1, symbol: 'BTC-USDT' }], 'BTCUSD'), 9, 'точное совпадение главнее псевдонима');
});

test('название пары: шлюз отдаёт base/quote числовыми кодами — имя берётся из символа', () => {
  assert.deepEqual(plain(Market.pair({ symbol: 'BTC-USDT', base: 1, quote: 100, category: 'crypto' })), ['BTC', 'USDT']);
  assert.deepEqual(plain(Market.pair({ symbol: 'EURUSD', base: 31, quote: 100, category: 'forex' })), ['EUR', 'USD']);
  assert.deepEqual(plain(Market.pair({ symbol: 'XAUUSD', base: 32, quote: 100, category: 'metals' })), ['XAU', 'USD']);
  assert.deepEqual(plain(Market.pair({ symbol: 'AAPL', base: 40, quote: 100, category: 'stocks' })), ['AAPL', 'USD']);
  assert.deepEqual(plain(Market.pair({ symbol: 'USOIL', base: 41, quote: 100, category: 'energy' })), ['USOIL', '']);
  assert.deepEqual(plain(Market.pair({ symbol: 'EURUSD', base: 'EUR', quote: 'USD' })), ['EUR', 'USD']);
  assert.deepEqual(plain(Market.pair(null)), ['', '']);
});
