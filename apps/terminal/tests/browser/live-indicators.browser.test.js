'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { openTerminal } = require('../helpers/browser');

/**
 * @param {any} page
 * @param {number} [ms]
 */
const settle = (page, ms) => page.evaluate((d) => new Promise((r) => setTimeout(r, d)), ms || 600);

/** @param {any} srv */
function candleRoutes(srv) {
  /** @param {URL} url */
  const candles = (url) => {
    const tf = Number(url.searchParams.get('tf')) || 60;
    const limit = Number(url.searchParams.get('limit')) || 300;
    srv.last = srv.last || {};
    if (srv.last[tf] == null) srv.last[tf] = Math.floor(Date.now() / 1000 / tf) * tf - 5 * tf;
    const last = srv.last[tf];
    const out = [];
    for (let i = 299; i >= 0; i -= 1) {
      const t = last - i * tf;
      const open = 6500000 + Math.round(Math.sin(t / tf / 7) * 30000);
      const close = open + ((t / tf) % 11 === 0 ? -26000 : 7000);
      out.push({ time: t, open, high: Math.max(open, close) + 4000, low: Math.min(open, close) - 4000, close, volume: 1000000 + ((t / tf) % 17) * 100000 });
    }
    const tailBars = out.slice(-limit);
    if (srv.reseed && limit <= 2) return tailBars.slice(0, -1).concat([srv.reseed]);
    return tailBars;
  };
  return { '/candles/1': candles, '/candles/2': candles };
}

const SNAPSHOT = `(() => {
  const raw = rawCandles;
  const closes = raw.map((c) => c.close);
  const ma = MA_SPEC.map(([n], k) => {
    const exp = Indicators.sma(closes, n);
    const data = overlaySeries[k].data();
    const last = data[data.length - 1];
    return { n, lastTime: last && last.time, lastValue: last && last.value, expected: exp[exp.length - 1],
      prevValue: data.length > 1 ? data[data.length - 2].value : null, prevExpected: exp[exp.length - 2], legend: maLegend.arrays[k][maLegend.arrays[k].length - 1] };
  });
  const main = mainSeries.data();
  const ha = Indicators.heikin(raw);
  const times = raw.map((c) => c.time);
  return {
    n: raw.length,
    lastTime: raw[raw.length - 1].time,
    lastClose: raw[raw.length - 1].close,
    sorted: times.every((t, i) => i === 0 || t > times[i - 1]),
    ma,
    lgMa: document.getElementById('lgMa').textContent,
    mainLast: main.slice(-4),
    haLast: ha.slice(-4).map((b) => ({ time: b.time, open: b.open, high: b.high, low: b.low, close: b.close })),
  };
})()`;

/** @param {any} page */
const snapshot = (page) => page.evaluate(SNAPSHOT);

/**
 * @param {any} page
 * @param {number} tf
 */
async function pickTf(page, tf) {
  await page.evaluate((v) => {
    /** @type {HTMLElement} */ (document.querySelector(`#tfs [data-tf="${v}"]`)).click();
  }, tf);
  await settle(page, 800);
}

/**
 * @param {any} term
 * @param {number} start
 * @param {number} tf
 * @param {number} bars
 */
async function pushBars(term, start, tf, bars) {
  let price = 6500000;
  for (let k = 0; k < bars; k += 1) {
    for (let j = 0; j < 3; j += 1) {
      price += ((k * 7 + j * 5) % 9 - 4) * 1500;
      term.push([{ type: 'trade', instrument: 1, price, qty: 10000 + j, side: j % 2 ? 'sell' : 'buy', time: (start + k * tf) * 1000 + j * 10 }]);
    }
    await settle(term.page, 120);
  }
  await settle(term.page, 600);
}

/**
 * @param {any} snap
 * @param {number} expectTime
 */
function assertMaFollows(snap, expectTime) {
  assert.ok(snap.sorted, 'время ряда строго растёт');
  assert.equal(snap.lastTime, expectTime, 'живая свеча вошла в ряд');
  for (const m of snap.ma) {
    assert.equal(m.lastTime, expectTime, `MA(${m.n}) дотянута до последнего бара`);
    assert.ok(Math.abs(m.lastValue - m.expected) < 1e-6, `MA(${m.n}) на живом баре: ${m.lastValue} против ${m.expected}`);
    assert.ok(Math.abs(m.prevValue - m.prevExpected) < 1e-6, `MA(${m.n}) на закрытом баре получила итог: ${m.prevValue} против ${m.prevExpected}`);
    assert.ok(Math.abs(m.legend - m.expected) < 1e-6, `легенда MA(${m.n}) на живом баре`);
  }
}

for (const tf of [1, 60]) {
  test(`tf=${tf}: MA следуют за живыми свечами через несколько интервалов`, async (t) => {
    const srv = {};
    const term = await openTerminal({ query: '?site=apex-ru', routes: candleRoutes(srv) });
    t.after(term.cleanup);
    await settle(term.page, 1500);
    await pickTf(term.page, tf);

    const before = await snapshot(term.page);
    assert.equal(before.lastTime, srv.last[tf]);
    const start = srv.last[tf];
    await pushBars(term, start, tf, 6);
    const after = await snapshot(term.page);

    assert.equal(after.n, before.n + 5, 'тот же бар заменён, пять новых добавлены');
    assertMaFollows(after, start + 5 * tf);
    assert.match(after.lgMa, /MA\(7\) \S/);
    assert.deepEqual(term.errors, [], term.errors.join(' | '));
  });
}

test('хейкин-аши продолжает цепочку от обновлённого ряда', async (t) => {
  const srv = {};
  const term = await openTerminal({ query: '?site=apex-ru', routes: candleRoutes(srv) });
  t.after(term.cleanup);
  await settle(term.page, 1500);
  await pickTf(term.page, 1);
  await term.page.evaluate(() => {
    /** @type {HTMLElement} */ (document.querySelector('#ctMenu [data-ct="heikin"]')).click();
  });
  await settle(term.page, 500);

  await pushBars(term, srv.last[1], 1, 5);
  const snap = await snapshot(term.page);
  assert.equal(snap.mainLast.length, 4);
  snap.mainLast.forEach((b, i) => {
    const h = snap.haLast[i];
    assert.equal(b.time, h.time);
    for (const f of ['open', 'high', 'low', 'close']) assert.ok(Math.abs(b[f] - h[f]) < 1e-6, `хейкин ${f} на ${b.time}: ${b[f]} против ${h[f]}`);
  });
  assertMaFollows(snap, srv.last[1] + 4);
  assert.deepEqual(term.errors, [], term.errors.join(' | '));
});

test('пересев с сервера авторитетен: замещает живой бар без дублей и без отката времени', async (t) => {
  /** @type {any} */
  const srv = {};
  const term = await openTerminal({ query: '?site=apex-ru', routes: candleRoutes(srv) });
  t.after(term.cleanup);
  await settle(term.page, 1500);
  await pickTf(term.page, 60);

  const start = srv.last[60];
  await pushBars(term, start, 60, 3);
  const live = await snapshot(term.page);
  const formingTime = start + 2 * 60;
  assert.equal(live.lastTime, formingTime);

  srv.reseed = { time: formingTime, open: 6400000, high: 6800000, low: 6300000, close: 6777700, volume: 9900000 };
  await settle(term.page, 2800);
  const after = await snapshot(term.page);

  assert.equal(after.n, live.n, 'пересев не добавил дубль');
  assert.equal(after.lastTime, formingTime, 'время не откатилось');
  assert.equal(after.lastClose, 67777);
  assertMaFollows(after, formingTime);

  srv.reseed = { time: formingTime - 60, open: 1, high: 1, low: 1, close: 1, volume: 1 };
  await settle(term.page, 2500);
  const stale = await snapshot(term.page);
  assert.equal(stale.lastTime, formingTime, 'старый бар с сервера ряд назад не откатывает');
  assert.equal(stale.lastClose, 67777);
  assert.deepEqual(term.errors, [], term.errors.join(' | '));
});
