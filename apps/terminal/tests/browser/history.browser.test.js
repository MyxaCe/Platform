'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { openTerminal } = require('../helpers/browser');

/**
 * @param {any} page
 * @param {number} [ms]
 */
const settle = (page, ms) => page.evaluate((d) => new Promise((r) => setTimeout(r, d)), ms || 600);

/**
 * Источник с конечной историей: `total` баров до «сейчас», `end` отдаёт бары
 * не позже указанного времени — как шлюз `/candles/{id}?tf=&limit=&end=`.
 * @param {{ total: number, requests: { id: string, tf: number, limit: number, end: number|null }[], hold?: Promise<void>|null }} srv
 */
function historyRoutes(srv) {
  /** @param {string} id */
  const candles = (id) => async (/** @type {URL} */ url) => {
    const tf = Number(url.searchParams.get('tf')) || 60;
    const limit = Number(url.searchParams.get('limit')) || 300;
    const endRaw = url.searchParams.get('end');
    const end = endRaw == null ? null : Number(endRaw);
    srv.requests.push({ id, tf, limit, end });
    if (end != null && srv.hold) await srv.hold;
    const last = Math.floor(Date.now() / 1000 / tf) * tf;
    const first = last - (srv.total - 1) * tf;
    const top = end == null ? last : Math.floor(end / tf) * tf;
    const out = [];
    for (let t = top; t >= first && out.length < limit; t -= tf) {
      const open = 6500000 + Math.round(Math.sin(t / tf / 7) * 30000);
      const close = open + ((t / tf) % 11 === 0 ? -26000 : 7000);
      out.push({ time: t, open, high: Math.max(open, close) + 4000, low: Math.min(open, close) - 4000, close, volume: 1000000 });
    }
    return out.reverse();
  };
  return { '/candles/1': candles('1'), '/candles/2': candles('2') };
}

/** @typedef {{ n: number, first: number, sorted: boolean, main: number, vol: number, from: number|null, firstVisibleTime: number|null }} ChartState */
const STATE = `(() => {
  const times = rawCandles.map((c) => c.time);
  const r = chart.timeScale().getVisibleLogicalRange();
  return {
    n: times.length,
    first: times[0],
    sorted: times.every((t, i) => i === 0 || t > times[i - 1]),
    main: mainSeries.data().length,
    vol: volume.data().length,
    from: r ? r.from : null,
    firstVisibleTime: r ? times[Math.max(0, Math.ceil(r.from))] : null,
  };
})()`;

/**
 * Тянем график мышью вправо — к более старым свечам.
 * @param {any} page
 * @param {number} times
 */
async function dragToPast(page, times) {
  const box = await page.$eval('#chart', (/** @type {HTMLElement} */ el) => { const r = el.getBoundingClientRect(); return { x: r.x, y: r.y, w: r.width, h: r.height }; });
  for (let k = 0; k < times; k += 1) {
    const y = box.y + box.h / 2;
    await page.mouse.move(box.x + box.w * 0.15, y);
    await page.mouse.down();
    await page.mouse.move(box.x + box.w * 0.85, y, { steps: 12 });
    await page.mouse.up();
    await settle(page, 400);
  }
}

test('история: прокрутка влево подгружает старые свечи страницами до начала истории', async (t) => {
  const srv = { total: 1100, requests: /** @type {any[]} */ ([]), hold: null };
  const term = await openTerminal({ query: '?site=apex-ru', routes: historyRoutes(srv) });
  t.after(term.cleanup);
  await settle(term.page, 1500);

  const start = /** @type {ChartState} */ (await term.page.evaluate(STATE));
  assert.equal(start.n, 300, 'сначала — одна страница');

  await dragToPast(term.page, 30);
  await settle(term.page, 800);
  const end = /** @type {ChartState} */ (await term.page.evaluate(STATE));
  assert.equal(end.n, 1100, 'подгружена вся история');
  assert.ok(end.sorted, 'по возрастанию, без дублей');
  assert.equal(end.main, 1100, 'основная серия перерисована');
  assert.equal(end.vol, 1100, 'объём перерисован');

  const older = srv.requests.filter((r) => r.id === '1' && r.end != null);
  assert.ok(older.length >= 2, `страниц истории: ${older.length}`);
  assert.equal(older[0].end, start.first - 1, 'следующая страница — до самой старой свечи');
  assert.ok(older.every((r) => r.limit <= 1000), 'не больше предела шлюза');
  const after = older.length;
  await dragToPast(term.page, 4);
  assert.equal(srv.requests.filter((r) => r.id === '1' && r.end != null).length, after, 'история кончилась — больше не спрашиваем');
});

test('история: при подгрузке видимая область не прыгает, а смена ТФ отбрасывает запоздавший ответ', async (t) => {
  /** @type {() => void} */
  let release = () => {};
  const srv = { total: 2000, requests: /** @type {any[]} */ ([]), hold: /** @type {Promise<void>|null} */ (null) };
  const term = await openTerminal({ query: '?site=apex-ru', routes: historyRoutes(srv) });
  t.after(term.cleanup);
  await settle(term.page, 1500);

  await dragToPast(term.page, 6);
  await settle(term.page, 800);
  const before = /** @type {ChartState} */ (await term.page.evaluate(STATE));
  assert.ok(before.n > 300, 'одна страница истории уже подгружена');
  await term.page.evaluate(() => chart.timeScale().setVisibleLogicalRange({ from: 40, to: 120 }));
  await settle(term.page, 200);
  const anchor = /** @type {ChartState} */ (await term.page.evaluate(STATE));
  await term.page.evaluate(() => chart.timeScale().setVisibleLogicalRange({ from: 5, to: 85 }));
  await settle(term.page, 1000);
  const grown = /** @type {ChartState} */ (await term.page.evaluate(STATE));
  assert.ok(grown.n > anchor.n, 'подгрузилась ещё страница');
  assert.ok(grown.firstVisibleTime <= anchor.firstVisibleTime && grown.from > 5, 'видимая область сдвинута на добавленные бары, а не осталась у нового начала');

  srv.hold = new Promise((r) => { release = r; });
  await term.page.evaluate(() => chart.timeScale().setVisibleLogicalRange({ from: 2, to: 80 }));
  await settle(term.page, 300);
  await term.page.evaluate(() => /** @type {HTMLElement} */ (document.querySelector('#tfs [data-tf="300"]')).click());
  await settle(term.page, 800);
  release();
  srv.hold = null;
  await settle(term.page, 800);
  const fresh = /** @type {ChartState} */ (await term.page.evaluate(STATE));
  assert.equal(fresh.n, 300, 'после смены ТФ — только новая первая страница, старая подгрузка выброшена');
  assert.ok(fresh.sorted);
});
