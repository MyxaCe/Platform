'use strict';
// ============================================================================
// Изменение SL/TP открытой позиции и ордера (17.11) — в настоящем браузере.
//
// Шлюз — заглушка (см. helpers/browser.js): здесь доказывается ФРОНТ — что
// кнопки появляются только под флагом шлюза, что окно подставляет минимум и
// подсвечивает слишком близкий уровень, что уходит ровно изменённое поле, что
// линии графика перерисованы, и что на телефоне всё это помещается в экран.
// Границу (отказы шлюза) проверяют тесты `levels::wiring` в Rust.
// ============================================================================
const test = require('node:test');
const assert = require('node:assert/strict');
const { openTerminal } = require('../helpers/browser');

const settle = (/** @type {any} */ page, /** @type {number} */ ms) => page.evaluate((d) => new Promise((r) => setTimeout(r, d)), ms || 600);

/** Заглушка с состоянием: PUT меняет уровни, следующий GET их отдаёт. */
function stubWorld(/** @type {{ edit?: boolean, reject?: any }} */ opt) {
  const deals = [
    { id: 7, instrument: 1, symbol: 'BTC-USDT', side: 'buy', qty: 1500, entry: 6480000, mark: 6500000, pnl: 30000, sl: 6420000, tp: null, price_decimals: 2, qty_decimals: 5 },
    { id: 9, instrument: 1, symbol: 'BTC-USDT', side: 'sell', qty: 500, entry: 6530000, mark: 6500100, pnl: 14950, sl: null, tp: 6440000, price_decimals: 2, qty_decimals: 5 },
  ];
  const pending = [
    { id: 12, instrument: 1, symbol: 'BTC-USDT', side: 'buy', qty: 1000, price: 6400000, sl: 6350000, tp: 6600000, price_decimals: 2, qty_decimals: 5 },
  ];
  /** @type {any[]} */
  const puts = [];
  const apply = (/** @type {any} */ row, /** @type {string} */ body) => {
    const b = JSON.parse(body || '{}');
    puts.push(b);
    if (opt.reject) return { __status: opt.reject.status, body: opt.reject.body };
    if ('sl' in b) row.sl = b.sl;
    if ('tp' in b) row.tp = b.tp;
    return { id: row.id, sl: row.sl, tp: row.tp, changed: true };
  };
  const routes = {
    '/account': () => ({
      opened: true, balance: 1000000, equity: 1044950, used_margin: 20000, free_margin: 1024950, open_pnl: 44950,
      model: 'hedging', leverage: 10, kind: 'demo', min_stop_bps: 50,
      ...(opt.edit === undefined ? {} : { sltp_edit: opt.edit }),
    }),
    '/deals': () => deals,
    '/pending': () => pending,
    '/deals/7/levels': (/** @type {any} */ _u, /** @type {any} */ _r, /** @type {string} */ body) => apply(deals[0], body),
    '/deals/9/levels': (/** @type {any} */ _u, /** @type {any} */ _r, /** @type {string} */ body) => apply(deals[1], body),
    '/pending/12/levels': (/** @type {any} */ _u, /** @type {any} */ _r, /** @type {string} */ body) => apply(pending[0], body),
  };
  return { routes, puts, deals, pending };
}

/** Открыть терминал с сессией; на телефоне — новой вкладкой с нужным экраном. */
async function open(/** @type {any} */ t, /** @type {any} */ world, /** @type {any} */ phone) {
  const term = await openTerminal({ query: '?site=apex-ru#handoff=h-lvl&account=demo&lang=ru', routes: world.routes });
  t.after(term.cleanup);
  if (phone) {
    const page = await term.browser.newPage();
    page.on('console', (/** @type {any} */ m) => { if (m.type() === 'error') term.errors.push(m.text()); });
    page.on('pageerror', (/** @type {any} */ e) => term.errors.push(`pageerror: ${e.message}`));
    await page.setViewport({ width: 390, height: 844, isMobile: true, hasTouch: true });
    await page.goto(term.origin + '/?site=apex-ru#handoff=h-lvl-m&account=demo&lang=ru', { waitUntil: 'load' });
    term.errors.length = 0;
    term.page = page;
  }
  await settle(term.page, 2000);
  return term;
}

const lvlButtons = (/** @type {any} */ page) => page.evaluate(() => document.querySelectorAll('.pane[data-pane="deals"] .deal-row .lvlbtn').length);

test('флаг шлюза выключен (или поля нет): SL/TP — текст, кнопок изменения нет', async (t) => {
  for (const edit of [false, undefined]) {
    const term = await open(t, stubWorld({ edit }));
    assert.equal(await lvlButtons(term.page), 0, `sltp_edit=${edit}`);
    const cell = await term.page.evaluate(() => document.querySelector('.pane[data-pane="deals"] .deal-row .c-sl').textContent);
    assert.equal(cell, '64200.00', 'уровень показан как был');
    assert.deepEqual(term.errors, []);
  }
});

test('флаг включён: окно подставляет минимум, подсвечивает близкий уровень и шлёт только изменённое', async (t) => {
  const world = stubWorld({ edit: true });
  const term = await open(t, world);
  const page = term.page;
  assert.equal(await lvlButtons(page), 4, 'по две ячейки у двух позиций');

  await page.click('.deal-row .lvlbtn[data-id="7"][data-f="sl"]');
  await settle(page, 300);
  const opened = await page.evaluate(() => ({
    sl: /** @type {HTMLInputElement} */ (document.getElementById('lvlSl')).value,
    tp: /** @type {HTMLInputElement} */ (document.getElementById('lvlTp')).value,
    ref: document.getElementById('lvlRef').textContent,
    focus: document.activeElement && document.activeElement.id,
  }));
  assert.deepEqual([opened.sl, opened.tp, opened.focus], ['64200.00', '', 'lvlSl']);
  // Покупку закрывают по bid 65 000.00; 0.5 % = 325.00.
  assert.match(opened.ref, /bid.*65,000\.00.*0\.50%.*325\.00/);

  // Ближе минимума: подсветка, сохранить нельзя, назван ближайший уровень.
  const sl = await page.$('#lvlSl');
  await sl.click({ clickCount: 3 });
  await sl.type('64900');
  await settle(page, 150);
  const bad = await page.evaluate(() => ({
    bad: document.getElementById('lvlSl').classList.contains('bad'),
    save: /** @type {HTMLButtonElement} */ (document.querySelector('.lvl-dlg__save')).disabled,
    msg: document.getElementById('lvlMsg').textContent,
  }));
  assert.ok(bad.bad, 'поле подсвечено');
  assert.ok(bad.save, 'сохранить нельзя');
  assert.match(bad.msg, /64,675\.00/, `ближайший допустимый: «${bad.msg}»`);

  // «Мин.» — уровень на минимуме с запасом на движение рынка (как тикет).
  await page.click('.lvl-f [data-act="min"][data-k="sl"]');
  await settle(page, 150);
  const minVal = await page.evaluate(() => /** @type {HTMLInputElement} */ (document.getElementById('lvlSl')).value);
  assert.equal(minVal, '64642.50', '325.00 + 10 % запаса от bid');
  await page.click('.lvl-dlg__save');
  await settle(page, 1200);

  assert.deepEqual(world.puts, [{ sl: 6464250 }], 'ушло только изменённое поле');
  const after = await page.evaluate(() => ({
    open: !!document.querySelector('.lvl-back'),
    cell: document.querySelector('.deal-row .lvlbtn[data-id="7"][data-f="sl"]').textContent.trim(),
    // Линии графика — из того же ответа /deals (currentDeals → redrawPriceLines).
    // eslint-disable-next-line no-undef
    lines: /** @type {any[]} */ (priceLines).map((l) => l.options()).filter((o) => o.title === 'SL').map((o) => o.price),
  }));
  assert.equal(after.open, false, 'окно закрылось');
  assert.equal(after.cell, '64642.50', 'таблица перерисована');
  assert.ok(after.lines.includes(64642.5), `линия SL на графике перерисована: ${after.lines}`);
  assert.ok(!after.lines.includes(64200), 'старой линии нет');
  assert.deepEqual(term.errors, []);
});

test('«Снять» уводит null, отказ шлюза показан словами и с уровнем шлюза', async (t) => {
  const world = stubWorld({ edit: true });
  const term = await open(t, world);
  const page = term.page;
  await page.click('.deal-row .lvlbtn[data-id="9"][data-f="tp"]');
  await settle(page, 300);
  await page.click('.lvl-f [data-act="clear"][data-k="tp"]');
  await page.click('.lvl-dlg__save');
  await settle(page, 1200);
  assert.deepEqual(world.puts, [{ tp: null }]);
  const cell = await page.evaluate(() => document.querySelector('.deal-row .lvlbtn[data-id="9"][data-f="tp"]').textContent.trim());
  assert.equal(cell, '—');

  const w2 = stubWorld({ edit: true, reject: { status: 422, body: { error: 'take_profit_too_close', limit: 6460000, min_stop_bps: 50, min_distance: 32501, reference: 6500100 } } });
  const t2 = await open(t, w2);
  await t2.page.click('.deal-row .lvlbtn[data-id="9"][data-f="tp"]');
  await settle(t2.page, 300);
  const tp = await t2.page.$('#lvlTp');
  await tp.click({ clickCount: 3 });
  await tp.type('64000');
  await t2.page.click('.lvl-dlg__save');
  await settle(t2.page, 600);
  const msg = await t2.page.evaluate(() => ({ text: document.getElementById('lvlMsg').textContent, open: !!document.querySelector('.lvl-back') }));
  assert.ok(msg.open, 'окно не закрылось на отказе');
  assert.match(msg.text, /Take Profit/);
  assert.doesNotMatch(msg.text, /take_profit_too_close/, 'не сырым кодом');
  assert.match(msg.text, /64,600\.00/, `уровень от шлюза: «${msg.text}»`);
});

test('ордер: отсчёт от цены ордера, запрос — на /pending/{id}/levels', async (t) => {
  const world = stubWorld({ edit: true });
  const term = await open(t, world);
  const page = term.page;
  await page.click('.bottom__tabs [data-pane="pending"]');
  await settle(page, 300);
  await page.click('.deal-row .lvlbtn[data-id="12"][data-f="tp"]');
  await settle(page, 300);
  const ref = await page.evaluate(() => document.getElementById('lvlRef').textContent);
  assert.match(ref, /64,000\.00.*320\.00/, `цена ордера и минимум от неё: «${ref}»`);
  const tp = await page.$('#lvlTp');
  await tp.click({ clickCount: 3 });
  await tp.type('66500');
  await page.keyboard.press('Enter');
  await settle(page, 1200);
  assert.deepEqual(world.puts, [{ tp: 6650000 }]);
  assert.equal(await page.evaluate(() => !!document.querySelector('.lvl-back')), false);
});

test('телефон: позиции карточкой — «Закрыть» и уровни в экране, окно снизу и в ширину экрана', async (t) => {
  const world = stubWorld({ edit: true });
  const term = await open(t, world, true);
  const page = term.page;
  await page.click('#mtabs [data-m="positions"]');
  await settle(page, 400);
  const layout = await page.evaluate(() => {
    const r = (/** @type {Element} */ el) => el.getBoundingClientRect();
    const row = document.querySelector('.pane[data-pane="deals"] .deal-row');
    return {
      vw: innerWidth,
      scroll: document.documentElement.scrollWidth,
      close: r(row.querySelector('.closebtn')).right,
      sl: r(row.querySelector('.lvlbtn[data-f="sl"]')).height,
      head: r(document.querySelector('.pane[data-pane="deals"] .deals__head')).height,
    };
  });
  assert.ok(layout.scroll <= layout.vw, 'страница не шире экрана');
  assert.ok(layout.close <= layout.vw, `«Закрыть» в экране: ${layout.close} ≤ ${layout.vw}`);
  assert.ok(layout.sl >= 36, `уровень — цель под палец: ${layout.sl}px`);
  assert.equal(layout.head, 0, 'заголовок таблицы на телефоне не нужен');

  await page.tap('.deal-row .lvlbtn[data-id="7"][data-f="sl"]');
  await settle(page, 400);
  const sheet = await page.evaluate(() => {
    const d = document.querySelector('.lvl-dlg').getBoundingClientRect();
    const save = document.querySelector('.lvl-dlg__save').getBoundingClientRect();
    return { w: Math.round(d.width), bottom: Math.round(d.bottom), vw: innerWidth, vh: innerHeight, save: save.height };
  });
  assert.equal(sheet.w, sheet.vw, 'окно во всю ширину');
  assert.equal(sheet.bottom, sheet.vh, 'прижато к низу экрана');
  assert.ok(sheet.save >= 44, 'кнопка под палец');
  await page.tap('.lvl-f [data-act="min"][data-k="sl"]');
  await page.tap('.lvl-dlg__save');
  await settle(page, 1200);
  assert.deepEqual(world.puts, [{ sl: 6464250 }]);
  assert.deepEqual(term.errors, []);
});
