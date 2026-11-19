'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadModule } = require('./helpers/load-module');

const LiveBars = loadModule('livebars.js', 'LiveBars');
const I = loadModule('indicators.js', 'Indicators');

/** @param {any} x */
const plain = (x) => JSON.parse(JSON.stringify(x));

/** @param {number} time @param {number} close */
function bar(time, close) {
  return { time, open: close - 1, high: close + 2, low: close - 2, close, volume: 3 };
}

/** @param {number} n */
function history(n) {
  const out = [];
  for (let i = 0; i < n; i += 1) out.push(bar(1000 + i, 100 + Math.sin(i / 3) * 5 + (i % 7)));
  return out;
}

function fakeSeries() {
  /** @type {{ time: number, value: number }[]} */
  const data = [];
  return {
    data,
    /** @param {{ time: number, value: number }[]} d */
    setData(d) { data.splice(0, data.length, ...d); },
    /** @param {{ time: number, value: number }} p */
    update(p) {
      const last = data[data.length - 1];
      if (last && p.time < last.time) throw new Error(`update назад: ${p.time} < ${last.time}`);
      if (last && p.time === last.time) data[data.length - 1] = p;
      else data.push(p);
    },
  };
}

test('тот же time заменяет последний бар', () => {
  const rows = history(5);
  const kind = LiveBars.upsert(rows, bar(1004, 500));
  assert.equal(kind, 'same');
  assert.equal(rows.length, 5);
  assert.equal(rows[4].close, 500);
});

test('новее последнего — добавляется', () => {
  const rows = history(5);
  assert.equal(LiveBars.upsert(rows, bar(1010, 7)), 'new');
  assert.equal(rows.length, 6);
  assert.equal(rows[5].time, 1010);
});

test('старше последнего — игнор, ряд не меняется', () => {
  const rows = history(5);
  const before = JSON.stringify(rows);
  assert.equal(LiveBars.upsert(rows, bar(1002, 1)), 'old');
  assert.equal(JSON.stringify(rows), before);
});

test('пустой ряд принимает первый бар', () => {
  const rows = [];
  assert.equal(LiveBars.upsert(rows, bar(5, 1)), 'new');
  assert.equal(rows.length, 1);
});

test('потолок длины обрезает ряд с начала', () => {
  const rows = history(10);
  for (let t = 1010; t < 1030; t += 1) LiveBars.upsert(rows, bar(t, 1), 12);
  assert.equal(rows.length, 12);
  assert.equal(rows[0].time, 1018);
  assert.equal(rows[11].time, 1029);
});

test('потолок по умолчанию — CAP', () => {
  const rows = [];
  for (let t = 0; t < LiveBars.CAP + 50; t += 1) LiveBars.upsert(rows, bar(t, 1));
  assert.equal(rows.length, LiveBars.CAP);
  assert.equal(rows[rows.length - 1].time, LiveBars.CAP + 49);
});

test('upsert копирует бар, а не хранит ссылку', () => {
  const rows = [];
  const b = bar(1, 1);
  LiveBars.upsert(rows, b);
  b.close = 99;
  assert.equal(rows[0].close, 1);
});

test('indexFrom — первый индекс со временем не раньше заданного', () => {
  const rows = history(5);
  assert.equal(LiveBars.indexFrom(rows, 1003), 3);
  assert.equal(LiveBars.indexFrom(rows, 999), 0);
  assert.equal(LiveBars.indexFrom(rows, 2000), 4);
});

test('tail пропускает пустые и нечисловые значения', () => {
  const pts = LiveBars.tail([null, 1, NaN, 3, Infinity, 5], [10, 11, 12, 13, 14, 15], 1);
  assert.deepEqual(plain(pts), [{ time: 11, value: 1 }, { time: 13, value: 3 }, { time: 15, value: 5 }]);
});

test('догон хвостом даёт тот же ряд, что полный пересчёт: MA, EMA, RSI, хейкин', () => {
  const rows = history(120);
  const calcs = {
    ma7: (cs) => I.sma(cs.map((c) => c.close), 7),
    ma99: (cs) => I.sma(cs.map((c) => c.close), 99),
    ema: (cs) => I.ema(cs.map((c) => c.close), 20),
    rsi: (cs) => I.rsi(cs.map((c) => c.close), 14),
  };
  const series = {};
  for (const k in calcs) {
    series[k] = fakeSeries();
    series[k].setData(LiveBars.tail(calcs[k](rows), rows.map((c) => c.time), 0));
  }
  let price = 110;
  for (let t = 1120; t < 1160; t += 1) {
    for (let j = 0; j < 3; j += 1) {
      price += ((t * 7 + j * 13) % 9) - 4;
      const n = rows.length;
      const prev = rows[n - 1].time;
      const kind = LiveBars.upsert(rows, bar(t, price));
      const from = kind === 'new' ? prev : t;
      const i0 = LiveBars.indexFrom(rows, from);
      const times = rows.map((c) => c.time);
      for (const k in calcs) for (const p of LiveBars.tail(calcs[k](rows), times, i0)) series[k].update(p);
    }
  }
  const times = rows.map((c) => c.time);
  for (const k in calcs) {
    const full = LiveBars.tail(calcs[k](rows), times, 0);
    assert.deepEqual(plain(series[k].data), plain(full), `ряд ${k} разошёлся с полным пересчётом`);
    assert.equal(series[k].data[series[k].data.length - 1].time, rows[rows.length - 1].time);
  }
  const ha = I.heikin(rows);
  const last = ha[ha.length - 1], prevHa = ha[ha.length - 2];
  assert.equal(last.open, (prevHa.open + prevHa.close) / 2, 'новый бар хейкина опирается на предыдущий');
});
