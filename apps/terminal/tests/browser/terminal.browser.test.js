'use strict';
// ============================================================================
// Браузерные проверки терминала (TRM-18).
//
// Здесь закрывается то, что модульными тестами закрыть нельзя в принципе и что
// до сих пор числилось «почти готовым»:
//   1) приложение вообще открывается — ни один зелёный модульный тест этого не
//      доказывает;
//   2) `drawings` рисует: настоящий <canvas>, настоящий 2D-контекст;
//   3) метка устаревания ВИДНА человеку, а не только выставлена атрибутом;
//   4) postMessage-половина контракта SSO;
//   5) след от неразобранного сообщения фида — правка TRM-16, которую нечем
//      было проверить: `app.js` в jsdom не поднимается.
//
// ГРАНИЦА, И ОНА ВАЖНЕЕ СПИСКА. Шлюз здесь — заглушка. Всё, что доказывают эти
// проверки, — про фронт. Про настоящий gateway, про Binance и про фид они не
// говорят ничего: скриншот интерфейса с котировками свидетельством работы фида
// не является. Половина SSO, которую они закрывают, — наша; родитель
// поддельный, и согласие настоящего кабинета с контрактом отсюда не следует
// (это по-прежнему TRM-04).
// ============================================================================
const test = require('node:test');
const assert = require('node:assert/strict');
const { openTerminal } = require('../helpers/browser');

/** Дать странице доработать опросы: они идут по таймерам, а не по событию. */
const settle = (page, ms) => page.evaluate((d) => new Promise((r) => setTimeout(r, d)), ms || 600);

/** Сколько непрозрачных пикселей на слое рисования. */
const ink = (page) => page.evaluate(() => {
  const c = document.querySelector('canvas.drawlayer');
  const d = c.getContext('2d').getImageData(0, 0, c.width, c.height).data;
  let n = 0;
  for (let i = 3; i < d.length; i += 4) if (d[i] !== 0) n += 1;
  return n;
});

/** Поставить горизонтальную линию: инструмент на одну точку, один щелчок. */
async function drawHLine(page) {
  await page.click('.drawbtn[data-tip="Horizontal line"]');
  const box = await page.evaluate(() => {
    const r = document.querySelector('canvas.drawlayer').getBoundingClientRect();
    return { x: r.x + r.width / 2, y: r.y + r.height / 2 };
  });
  await page.mouse.click(box.x, box.y);
  await settle(page, 300);
}

// ---- 1. Приложение открывается ---------------------------------------------

test('терминал открывается без единой ошибки в консоли', async (t) => {
  // Зелёный набор модульных тестов этого не доказывает: у соседей один
  // клиентский импорт уронил всю админку при 1282 зелёных тестах.
  const term = await openTerminal({ query: '?site=apex-ru' });
  t.after(term.cleanup);
  await settle(term.page, 1500);

  const built = await term.page.evaluate(() => ({
    watch: document.querySelectorAll('.irow').length,
    book: document.querySelectorAll('.brow').length,
    stats: document.querySelectorAll('.stat').length,
    tabs: document.querySelectorAll('.bottom__tabs [data-pane]').length,
    canvas: !!document.querySelector('canvas.drawlayer'),
  }));

  assert.deepEqual(term.errors, [], 'страница обязана молчать в консоль');
  assert.equal(built.watch, 2, 'список инструментов построен');
  assert.ok(built.book > 0, 'стакан построен');
  assert.equal(built.stats, 3, 'полоса статистики построена');
  assert.equal(built.tabs, 4, 'нижняя панель построена');
  assert.ok(built.canvas, 'слой рисования создан');
});

test('инструмент без котировки рисует прочерк во всех пяти полях', async (t) => {
  // Пятое поле, `change`, врало дольше четырёх остальных: шлюз отдавал 0
  // вместо «неизвестно», и строка рисовала зелёное «+0.00%» рядом с четырьмя
  // честными прочерками.
  const term = await openTerminal({ query: '?site=apex-ru' });
  t.after(term.cleanup);
  await settle(term.page, 1200);

  const row = await term.page.evaluate(() => {
    const el = document.querySelector('.irow[data-id="2"]');
    const txt = (s) => el.querySelector(s).textContent.trim();
    return {
      change: txt('.c-change'),
      sell: txt('.c-sell'),
      buy: txt('.c-buy'),
      high: txt('.c-high'),
      cls: el.querySelector('.c-change').className,
    };
  });

  assert.equal(row.change, '—');
  assert.equal(row.sell, '—');
  assert.equal(row.buy, '—');
  assert.equal(row.high, '—');
  assert.ok(!/\bup\b|\bdown\b/.test(row.cls), 'без направления строку не красим');
});

// ---- 2. Рисование на настоящем canvas ---------------------------------------

test('инструмент рисования кладёт пиксели на canvas', async (t) => {
  // jsdom без нативного пакета `canvas` отдаёт null на getContext('2d'), то
  // есть этот модуль не исполнялся в тестах ни разу.
  const term = await openTerminal({ query: '?site=apex-ru' });
  t.after(term.cleanup);
  await settle(term.page, 1500);

  assert.equal(await ink(term.page), 0, 'до рисования слой пуст');

  await drawHLine(term.page);

  const after = await ink(term.page);
  assert.ok(after > 0, `после щелчка на слое должны появиться пиксели, их ${after}`);
});

test('смена инструмента стирает рисунки: якоря к чужому ряду не относятся', async (t) => {
  // Путь настоящий: `clear()` зовёт сам хост при перезагрузке данных графика.
  // Дёргать его из теста значило бы проверить вызов, а не проводку.
  const term = await openTerminal({ query: '?site=apex-ru' });
  t.after(term.cleanup);
  await settle(term.page, 1500);

  await drawHLine(term.page);
  assert.ok(await ink(term.page) > 0, 'предпосылка: что-то нарисовано');

  await term.page.click('.irow[data-id="2"]');
  await settle(term.page, 1500);

  assert.equal(await ink(term.page), 0, 'рисунки прошлого инструмента обязаны уйти');
});

// ---- 3. Метка устаревания видна человеку ------------------------------------

test('метка устаревания видна человеку, а не только выставлена атрибутом', async (t) => {
  // В jsdom проверялось, что виджет выставляет `data-stale`. Видна ли метка —
  // проверить было нечем: стилей и раскладки там нет.
  let fail = false;
  const fresh = [{ tf: 60, change: 0.42 }, { tf: 300, change: -1.1 }, { tf: 3600, change: null }];
  const down = () => ({ __status: 503, body: { error: 'down' } });

  const term = await openTerminal({
    query: '?site=apex-ru',
    routes: { '/stats/1': () => (fail ? down() : fresh), '/stats/2': () => (fail ? down() : fresh) },
  });
  t.after(term.cleanup);
  await settle(term.page, 1500);

  const before = await term.page.evaluate(
    () => document.getElementById('statbar').getAttribute('data-stale'),
  );
  assert.equal(before, null, 'пока всё хорошо — метки нет');

  // Ждём ЕСТЕСТВЕННЫЙ опрос (5 с), а не дёргаем refresh: проверяется поведение
  // виджета на своём таймере, а не наш вызов.
  fail = true;
  await settle(term.page, 6500);

  const stale = await term.page.evaluate(() => {
    const bar = document.getElementById('statbar');
    const val = bar.querySelector('.stat__v');
    return {
      stale: bar.getAttribute('data-stale'),
      badge: getComputedStyle(bar, '::after').content,
      valueOpacity: Number(getComputedStyle(val).opacity),
      stillThere: val.textContent.trim(),
      title: bar.title,
    };
  });

  assert.equal(stale.stale, 'malformed', 'не-2xx доходит до виджета как «ответ не тот»');
  assert.match(stale.badge, /ОТВЕТ НЕ ТОТ/, 'плашка нарисована и несёт текст причины');
  assert.ok(stale.valueOpacity < 0.6, `числа обязаны погаснуть, прозрачность ${stale.valueOpacity}`);
  assert.notEqual(stale.stillThere, '', 'но остаться: они были верны на момент чтения');
  assert.match(stale.title, /не обновляется/);
});

// ---- 4. postMessage-половина SSO --------------------------------------------

/** Родитель: встраивает терминал и ведёт журнал полученных сообщений. */
const parentPage = (src) => `<!doctype html><html><head><meta charset="utf-8"></head><body>
<script>
  window.__got = [];
  window.addEventListener('message', (e) => { window.__got.push(e.data); });
  window.__send = (m) => document.getElementById('t').contentWindow.postMessage(m, '*');
</script>
<iframe id="t" src="${src}" style="width:1200px;height:700px;border:0"></iframe>
</body></html>`;

/**
 * Поднять терминал ВНУТРИ родителя.
 *
 * Адрес iframe абсолютный и всегда на `127.0.0.1` — именно это позволяет
 * открыть самого родителя по `localhost` и получить настоящий чужой origin,
 * который браузер проставит сам. Подделать origin из страницы нечем, поэтому
 * вне браузера эта граница непроверяема.
 */
async function withParent(t, opts) {
  const o = opts || {};
  const term = await openTerminal(Object.assign({}, o, { pages: { '/parent.html': 'готовится' } }));
  t.after(term.cleanup);

  const host = o.foreign ? term.origin.replace('127.0.0.1', 'localhost') : term.origin;
  await term.page.goto(`${host}/parent.html`, { waitUntil: 'domcontentloaded' });
  await term.page.setContent(parentPage(`${term.origin}/?site=apex-ru`), { waitUntil: 'load' });
  await settle(term.page, 2000);
  return term;
}

test('родитель передаёт handoff-токен — терминал меняет его на сессию', async (t) => {
  // Бэкендовая половина прогонялась живьём (TRM-04); эта — нет, и именно из-за
  // неё пользователь мог не войти при безупречном бэкенде.
  const term = await withParent(t);

  await term.page.evaluate(() => window.__send({ type: 'sso.token', token: 'handoff-abc' }));
  await settle(term.page, 1000);

  const minted = term.seen.find((r) => r.path === '/v1/session' && r.method === 'POST');
  assert.ok(minted, 'терминал обязан сходить за сессией');
  assert.match(minted.body, /handoff-abc/, 'в обмен уходит именно полученный токен');

  const authed = term.seen.filter((r) => r.auth && r.auth.includes('session-for-handoff-abc'));
  assert.ok(authed.length > 0, 'после обмена торговые ручки зовутся с сессией');
});

test('сообщение с чужого origin игнорируется', async (t) => {
  const term = await withParent(t, { foreign: true });

  await term.page.evaluate(() => window.__send({ type: 'sso.token', token: 'handoff-foreign' }));
  await settle(term.page, 1000);

  const minted = term.seen.find((r) => r.path === '/v1/session' && r.method === 'POST');
  assert.equal(minted, undefined, 'за сессией по чужому сообщению не ходим');
});

test('терминал просит новый токен заранее, а не после истечения', async (t) => {
  // Просьба уходит за 2 минуты до истечения: при TTL 121 с — примерно через 1 с.
  const term = await withParent(t, { sessionTtl: 121 });

  await term.page.evaluate(() => window.__send({ type: 'sso.token', token: 'handoff-ttl' }));
  await settle(term.page, 3000);

  const asked = await term.page.evaluate(
    () => window.__got.filter((m) => m && m.type === 'token.refresh.request').length,
  );
  assert.ok(asked > 0, 'терминал обязан попросить новый токен до истечения');
});

test('sso.logout гасит сессию', async (t) => {
  const term = await withParent(t);

  await term.page.evaluate(() => window.__send({ type: 'sso.token', token: 'handoff-bye' }));
  await settle(term.page, 1000);
  await term.page.evaluate(() => window.__send({ type: 'sso.logout' }));
  await settle(term.page, 1000);

  const out = term.seen.some((r) => r.path === '/v1/session/logout' && r.method === 'POST');
  assert.ok(out, 'выход обязан дойти до шлюза');
});

// ---- 5. Битое сообщение фида оставляет след ---------------------------------

test('сообщение фида, которое не разобралось, оставляет след, а не исчезает', async (t) => {
  // Мягкий разбор означает «не падает», а не «исчезает без следа».
  const term = await openTerminal({ query: '?site=apex-ru' });
  t.after(term.cleanup);
  await settle(term.page, 1500);

  const before = await term.page.evaluate(() => document.getElementById('status').title);
  assert.ok(!/не разобрано/.test(before), 'до битого сообщения счёта нет');

  term.push('{это не JSON');
  term.push('{тоже не JSON');
  await settle(term.page, 800);

  const after = await term.page.evaluate(() => document.getElementById('status').title);
  assert.match(after, /не разобрано сообщений: 2/, `в подсказке индикатора нет счёта: «${after}»`);
});

test('здоровое сообщение фида счётчик не трогает', async (t) => {
  // Иначе счётчик говорил бы о потоке, а не о поломке, и его перестали бы
  // читать — ровно так же, как молчание.
  const term = await openTerminal({ query: '?site=apex-ru' });
  t.after(term.cleanup);
  await settle(term.page, 1500);

  term.push([{ type: 'trade', instrument: 1, price: 6500000, qty: 100, side: 'buy' }]);
  await settle(term.page, 800);

  const title = await term.page.evaluate(() => document.getElementById('status').title);
  assert.ok(!/не разобрано/.test(title), `здоровое сообщение не должно считаться поломкой: «${title}»`);
});
