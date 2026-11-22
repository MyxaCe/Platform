'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { openTerminal } = require('../helpers/browser');

const settle = (/** @type {any} */ page, /** @type {number} */ ms) => page.evaluate((/** @type {number} */ d) => new Promise((r) => setTimeout(r, d)), ms || 600);

const ALL = ['ru', 'en', 'de', 'fr', 'es'];

/**
 * @param {string[]} languages
 * @param {string} defaultLanguage
 */
const brandRoutes = (languages, defaultLanguage) => ({
  '/v1/cms/brand': () => ({ name: 'Apex', primaryColor: '#d4a437', languages, defaultLanguage }),
  '/account': () => ({
    opened: true, balance: 1234567, equity: 1235801, used_margin: 20000, free_margin: 1215801, open_pnl: 1234,
    model: 'hedging', leverage: 100, kind: 'real', min_stop_bps: 50, commission_bps: 5, markup_bps: 0,
    margin_level_pct: 6179, margin_call_level_pct: 100, stop_out_level_pct: 50,
  }),
  '/deals': () => [
    { id: 7, instrument: 1, symbol: 'BTC-USDT', side: 'buy', qty: 1500, entry: 6480000, mark: 6500000, pnl: 30000, sl: 6420000, tp: null, price_decimals: 2, qty_decimals: 5, commission: 120 },
  ],
});

/**
 * @param {any} t
 * @param {string} lang
 * @param {{ width: number, height: number, isMobile?: boolean, hasTouch?: boolean }} viewport
 * @param {string[]} [languages]
 * @param {string} [def]
 */
async function open(t, lang, viewport, languages, def) {
  const query = `?site=apex-ru&lang=${lang}#handoff=h-i18n&account=real`;
  const term = await openTerminal({
    query,
    config: { brandUrl: '{origin}/v1/cms/brand' },
    routes: brandRoutes(languages || ALL, def || 'ru'),
  });
  t.after(term.cleanup);
  const page = await term.browser.newPage();
  page.on('console', (/** @type {any} */ m) => { if (m.type() === 'error') term.errors.push(m.text()); });
  page.on('pageerror', (/** @type {any} */ e) => term.errors.push(`pageerror: ${e.message}`));
  await page.setViewport(viewport);
  await page.goto(`${term.origin}/${query}`, { waitUntil: 'load' });
  await settle(page, 1500);
  return { ...term, page };
}

/**
 * @param {any} page
 * @param {string[]} selectors
 * @param {boolean} [inViewport]
 */
function overflow(page, selectors, inViewport) {
  return page.evaluate((/** @type {string[]} */ sels, /** @type {boolean} */ vp) => {
    const out = [];
    for (const sel of sels) {
      for (const el of /** @type {HTMLElement[]} */ ([...document.querySelectorAll(sel)])) {
        if (!el.getClientRects().length) continue;
        const r = el.getBoundingClientRect();
        if (r.width === 0 || r.height === 0) continue;
        const range = document.createRange();
        range.selectNodeContents(el);
        const tr = range.getBoundingClientRect();
        const text = (el.textContent || '').trim();
        if (!text) continue;
        const why = [];
        if (el.scrollWidth > el.clientWidth + 1) why.push(`scroll ${el.scrollWidth}>${el.clientWidth}`);
        if (tr.bottom > r.bottom + 1 || tr.top < r.top - 1) why.push(`text rows ${Math.round(tr.top)}..${Math.round(tr.bottom)} box ${Math.round(r.top)}..${Math.round(r.bottom)}`);
        if (tr.right > r.right + 1 || tr.left < r.left - 1) why.push(`text ${Math.round(tr.left)}..${Math.round(tr.right)} box ${Math.round(r.left)}..${Math.round(r.right)}`);
        if (vp && (r.right > window.innerWidth + 1 || r.left < -1)) why.push(`viewport ${Math.round(r.right)}>${window.innerWidth}`);
        const p = el.parentElement && el.parentElement.getBoundingClientRect();
        if (vp && p && (r.right > p.right + 1 || r.left < p.left - 1)) why.push(`parent ${Math.round(r.left)}..${Math.round(r.right)} in ${Math.round(p.left)}..${Math.round(p.right)}`);
        if (why.length) out.push(`${sel} «${text}»: ${why.join(', ')}`);
      }
    }
    return out;
  }, selectors, !!inViewport);
}

const PHONE = { width: 390, height: 844, isMobile: true, hasTouch: true };
const DESKTOP = { width: 1440, height: 900 };

const FORM = ['.deal__tabs > button', '.side__b', '.orderbtn', '.orderbtn__t', '.lbl', '.ubtn', '.shares button', '.deal__acct > span', '#stopsHint', '#stopsReset', '#sizeEq', '#marginEq', '#feeInfo', '.inp--static'];

for (const lang of ['de', 'fr', 'es']) {
  test(`телефон, ${lang}: вкладки, тикет и позиции не обрезаны и не шире экрана`, async (t) => {
    const term = await open(t, lang, PHONE);
    const { page } = term;
    assert.equal(await page.evaluate(() => document.documentElement.lang), lang);
    assert.deepEqual(await overflow(page, ['#mtabs button', '.lang button', '.btn-top', '#status', '.kind-badge', '.tbbtn'], true), []);
    assert.deepEqual(await overflow(page, ['.ih-l']), []);
    await page.click('#mtabs [data-m="order"]');
    await settle(page, 400);
    assert.deepEqual(await overflow(page, FORM, true), []);
    await page.click('#mtabs [data-m="positions"]');
    await settle(page, 400);
    assert.deepEqual(await overflow(page, ['.bottom__tabs > span', '.closebtn']), []);
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), 'страница не шире экрана телефона');
    assert.deepEqual(term.errors, []);
  });
}

for (const lang of ['de', 'fr', 'es']) {
  test(`десктоп, ${lang}: шапка, тулбар, тикет, нижняя панель, стакан и сделки не обрезаны`, async (t) => {
    const term = await open(t, lang, DESKTOP);
    const { page } = term;
    const parts = ['.metric', '.metric__l', '.kind-badge', '.chip', '.ih-l', '.ih-source', '.lang button', '.btn-top', '.tbbtn', '.bottom__tabs > span', '.ob__title > *', '.trades__tabs > *', '.trades__head > span', '.closebtn', ...FORM];
    assert.deepEqual(await overflow(page, parts, true), []);
    assert.ok(await page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth), 'страница не шире окна');
    assert.deepEqual(term.errors, []);
  });
}

test('язык из ссылки проверяется по brand: только языки сайта в переключателе, вне списка — язык сайта по умолчанию', async (t) => {
  const term = await open(t, 'fr', DESKTOP, ['ru', 'de', 'es'], 'es');
  const ui = await term.page.evaluate(() => ({
    lang: document.documentElement.lang,
    buttons: [...document.querySelectorAll('.lang [data-lang]')].map((b) => /** @type {HTMLElement} */ (b).dataset.lang),
    active: document.querySelector('.lang .active')?.getAttribute('data-lang'),
    balance: document.querySelector('[data-i18n="acct.balance"]').textContent,
  }));
  assert.deepEqual(ui, { lang: 'es', buttons: ['ru', 'de', 'es'], active: 'es', balance: 'Saldo' });
  await term.page.click('.lang [data-lang="de"]');
  await settle(term.page, 600);
  const de = await term.page.evaluate(() => ({
    balance: document.querySelector('[data-i18n="acct.balance"]').textContent,
    money: document.getElementById('mBalance').textContent.replace(/\s/g, ' '),
    book: document.querySelector('.ob__head').textContent,
  }));
  assert.equal(de.balance, 'Kontostand');
  assert.equal(de.money, '12.345,67 $');
  assert.match(de.book, /Markttiefe/);
  assert.deepEqual(term.errors, []);
});

test('brand недоступен: de из ссылки — английский, в переключателе RU и EN', async (t) => {
  const term = await openTerminal({ query: '?site=apex-ru&lang=de' });
  t.after(term.cleanup);
  await settle(term.page, 1200);
  const ui = await term.page.evaluate(() => ({
    lang: document.documentElement.lang,
    buttons: [...document.querySelectorAll('.lang [data-lang]')].map((b) => /** @type {HTMLElement} */ (b).dataset.lang),
    balance: document.querySelector('[data-i18n="acct.balance"]').textContent,
  }));
  assert.deepEqual(ui, { lang: 'en', buttons: ['ru', 'en'], balance: 'Balance' });
  assert.deepEqual(term.errors, []);
});
