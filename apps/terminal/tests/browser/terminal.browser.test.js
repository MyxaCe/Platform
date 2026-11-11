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
  const c = /** @type {HTMLCanvasElement} */ (document.querySelector('canvas.drawlayer'));
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
  assert.equal(built.tabs, 5, 'нижняя панель построена: позиции, ордера, история, баланс, сигналы');
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

  // 503 — это ОТКАЗ СОСЕДА, а не «ответ не тот»: разные неполадки у разных
  // людей (КОНТРАКТЫ §4г). Раньше виджет называл это `malformed`, потому что
  // `api.js` схлопывал всё в `null`.
  assert.equal(stale.stale, 'http-error', 'не-2xx зовётся http-error');
  assert.match(stale.badge, /ОТКАЗ ИСТОЧНИКА/, 'плашка нарисована и несёт текст причины');
  assert.ok(stale.valueOpacity < 0.6, `числа обязаны погаснуть, прозрачность ${stale.valueOpacity}`);
  assert.notEqual(stale.stillThere, '', 'но остаться: они были верны на момент чтения');
  assert.match(stale.title, /не обновляется/);
});

test('оборванная сеть зовётся network, а не отказом соседа (КОНТРАКТЫ §4г)', async (t) => {
  // Мутация «network → http-error» пережила весь набор: ветку сети не
  // исполнял никто. Здесь запрос рвётся по-настоящему — браузер отказывает в
  // соединении, `fetch` отклоняется, — и это единственный способ отличить
  // «наша сеть не донесла» от «сосед ответил отказом», не подменяя `fetch`.
  const term = await openTerminal({ query: '?site=apex-ru' });
  t.after(term.cleanup);
  await settle(term.page, 1500);

  await term.page.setRequestInterception(true);
  term.page.on('request', (req) => {
    if (req.url().includes('/stats/')) req.abort('connectionrefused').catch(() => {});
    else req.continue().catch(() => {});
  });

  await settle(term.page, 6500); // ждём естественный опрос полосы статистики

  const stale = await term.page.evaluate(() => {
    const bar = document.getElementById('statbar');
    return {
      stale: bar.getAttribute('data-stale'),
      badge: getComputedStyle(bar, '::after').content,
      title: bar.title,
    };
  });

  assert.equal(stale.stale, 'network', 'оборванная сеть — это network');
  assert.match(stale.badge, /НЕТ СВЯЗИ/);
  assert.match(stale.title, /не ответил/);
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

// ---- 6. Умолчания на границах: цена и время (TRM-24) -----------------------

/**
 * Состояние «цена ещё неизвестна» — то, в котором терминал открывается.
 *
 * Добраться до него переключением инструмента не вышло, и это выяснилось
 * прогоном: выбрав инструмент, терминал берёт цену из последней свечи, а при
 * пустых свечах переключение вовсе не доводится до конца — `selected` остаётся
 * прежним, и проверка смотрела на чужой инструмент, показывая «≈ $650.01».
 *
 * Поэтому цены нет у инструмента, который выбран ПРИ ОТКРЫТИИ: ни тикера, ни
 * свечей, ни стакана. Это и есть честный случай — первые секунды работы и
 * любой отказ источника свечей.
 */
const PRICELESS = {
  '/instruments': () => [
    {
      id: 1, symbol: 'BTC-USDT', base: 'BTC', quote: 'USDT',
      price_decimals: 2, qty_decimals: 5,
      last: null, bid: null, ask: null, high: null, change: null,
    },
  ],
  '/candles/1': () => [],
  '/depth/1': () => ({ price_decimals: 2, qty_decimals: 5, bids: [], asks: [] }),
};

test('неизвестная цена даёт прочерк в эквиваленте размера, а не «≈ 0»', async (t) => {
  // Панель ввода ордера. Ноль читается как «ноль объёма», а не «цены не знаем»,
  // и по нему человек принимает решение о деньгах.
  const term = await openTerminal({ query: '?site=apex-ru', routes: PRICELESS });
  t.after(term.cleanup);
  await settle(term.page, 1800);

  const shown = await term.page.evaluate(() => {
    const el = document.getElementById('sizeEq');
    return { text: el.textContent, title: el.title };
  });

  assert.match(shown.text, /—/, `ожидали прочерк, получили «${shown.text}»`);
  assert.doesNotMatch(shown.text, /≈ 0|\$0/, 'ноль здесь — утверждение о величине');
  assert.match(shown.title, /неизвестна/);

  // Регресс на то, что нашлось этой же проверкой: панель сделки звала
  // `it.change.toFixed(2)` без защиты. Когда шлюз начал честно отдавать null,
  // она падала целиком и обрывала выбор инструмента — а снаружи это выглядело
  // как «терминал не переключается».
  assert.deepEqual(term.errors, [], 'без цены страница обязана работать, а не падать');
});

test('цена ноль — это не цена, а отсутствие цены', async (t) => {
  // Найдено мутацией: ветка «разобрали, но величина негодная» фикстурой с
  // `last: null` не исполнялась — там выход происходит раньше. А ноль здесь
  // самый опасный из возможных входов: по марк-цене считают P&L и маржу, и
  // «цена 0» у нас уже была отдельным дефектом в потоке котировок.
  const term = await openTerminal({
    query: '?site=apex-ru',
    routes: {
      '/instruments': () => [{
        id: 1, symbol: 'BTC-USDT', base: 'BTC', quote: 'USDT',
        price_decimals: 2, qty_decimals: 5,
        last: 0, bid: null, ask: null, high: null, change: null,
      }],
      '/candles/1': () => [],
      '/depth/1': () => ({ price_decimals: 2, qty_decimals: 5, bids: [], asks: [] }),
    },
  });
  t.after(term.cleanup);
  await settle(term.page, 1800);

  const shown = await term.page.evaluate(() => document.getElementById('sizeEq').textContent);
  assert.match(shown, /—/, `ноль обязан читаться как «цены нет», получили «${shown}»`);
});

test('смена единицы при неизвестной цене не стирает введённый объём', async (t) => {
  // Пересчёт через p = 0 молча обнулял поле: человек набрал объём, переключил
  // единицу и потерял его. Это хуже неверной подписи — это правка ввода.
  const term = await openTerminal({ query: '?site=apex-ru', routes: PRICELESS });
  t.after(term.cleanup);
  await settle(term.page, 1800);

  await term.page.evaluate(() => {
    const i = /** @type {HTMLInputElement} */ (document.getElementById('lot'));
    i.value = '0.750';
  });
  await term.page.click('#unitUsd');
  await settle(term.page, 400);

  const after = await term.page.evaluate(
    () => /** @type {HTMLInputElement} */ (document.getElementById('lot')).value,
  );
  assert.equal(after, '0.750', 'введённое число обязано остаться нетронутым');
});

test('цена известна — эквивалент считается и пересчёт работает', async (t) => {
  // Зеркало двух предыдущих: отказ касается неизвестной цены, а не функции
  // вообще. Без этой пары «никогда не считать» выглядело бы так же зелено.
  const term = await openTerminal({ query: '?site=apex-ru' });
  t.after(term.cleanup);
  await settle(term.page, 1800);

  const shown = await term.page.evaluate(() => document.getElementById('sizeEq').textContent);
  assert.doesNotMatch(shown, /—/, `для инструмента с ценой ожидали число: «${shown}»`);
  assert.match(shown, /\d/);
});

test('сессия без названного срока жизни не принимается и не долбит кабинет', async (t) => {
  // Было `(d.expires_in || 0)`: пустой срок превращался в «истекла только
  // что», и терминал просил новый токен РАЗ В СЕКУНДУ. Умолчание на границе
  // времени не показывает неправду, а производит работу.
  const term = await withParent(t, { routes: { '/v1/session': () => ({ token: 'сессия-без-срока' }) } });

  await term.page.evaluate(() => window.__send({ type: 'sso.token', token: 'handoff-no-ttl' }));
  await settle(term.page, 3500);

  const asked = await term.page.evaluate(
    () => window.__got.filter((m) => m && m.type === 'token.refresh.request').length,
  );
  assert.equal(asked, 0, `шторма запросов быть не должно, их ${asked}`);

  const authed = term.seen.filter((r) => r.auth && r.auth.includes('сессия-без-срока'));
  assert.equal(authed.length, 0, 'сессия без срока не принимается');
});

// ---- 9. Незакрытый бар называет себя незакрытым (TRM-33) -------------------

test('пока бар не закрыт, легенда говорит об этом, а живые сделки не роняют график', async (t) => {
  // Условие решения по TRM-33: гистограмма объёма теперь обновляется живыми
  // сделками, а индикаторы считаются по закрытым барам. Расхождение на один
  // бар допустимо только названное — иначе оно читается как поломка.
  //
  // Второй половиной проверяется, что живое обновление серии объёма не роняет
  // график: ошибка порядка времени в Lightweight Charts — исключение, а не
  // кривая картинка, и молча её не заметить.
  const term = await openTerminal({ query: '?site=apex-ru' });
  t.after(term.cleanup);
  await settle(term.page, 1500);

  term.push([{ type: 'trade', instrument: 1, price: 6500000, qty: 100, side: 'buy' }]);
  term.push([{ type: 'trade', instrument: 1, price: 6501000, qty: 250, side: 'sell' }]);
  await settle(term.page, 800);

  const shown = await term.page.evaluate(() => document.getElementById('lgTime').textContent);
  assert.match(shown, /не закрыт/, `легенда обязана назвать бар незакрытым, а показывает «${shown}»`);

  assert.deepEqual(term.errors, [], `живые сделки не должны ронять график: ${term.errors.join(' | ')}`);
});

test('хейкин-аши живёт так же, как обычные свечи: бар обновляется и называет себя незакрытым (TRM-34)', async (t) => {
  // Режим, который переключает пользователь, — отдельный путь данных, а не
  // настройка показа. Здесь выход стоял до отрисовки и до легенды: график не
  // обновлялся живьём вовсе, и пометка не появлялась никогда. Условие решения,
  // выполненное в одном режиме из пяти, не выполнено.
  const term = await openTerminal({ query: '?site=apex-ru' });
  t.after(term.cleanup);
  await settle(term.page, 1500);

  await term.page.evaluate(() => {
    /** @type {HTMLElement} */ (document.querySelector('#ctMenu [data-ct="heikin"]')).click();
  });
  await settle(term.page, 500);

  // Таймфрейм — минута. Заглушка отдаёт бары с шагом 60 с независимо от
  // запрошенного tf, а по умолчанию терминал стоит на часе: живая сделка
  // попадала бы в интервал РАНЬШЕ последнего бара и отбрасывалась бы — верно
  // по существу, но проверяли бы мы тогда заглушку, а не терминал.
  await term.page.evaluate(() => {
    /** @type {HTMLElement} */ (document.querySelector('#tfs [data-tf="60"]')).click();
  });
  await settle(term.page, 700);

  const legend = () => term.page.evaluate(() => ({
    time: document.getElementById('lgTime').textContent,
    ohlc: document.getElementById('lgOhlc').textContent,
  }));

  // Снимок ДО сделки обязателен. Пометка «бар не закрыт» появляется уже при
  // загрузке — последний бар REST тоже незакрыт, — поэтому её наличие само по
  // себе живости НЕ доказывает: первая версия этого теста проверяла только её
  // и пережила обе мутации. Живость — это ИЗМЕНЕНИЕ после сделки.
  const before = await legend();

  // Две сделки разной цены в одном интервале: тогда у свечи хейкина закрытие
  // не совпадает с сырым (оно среднее по четырём), и подмену видно.
  term.push([{ type: 'trade', instrument: 1, price: 7000000, qty: 100, side: 'buy' }]);
  term.push([{ type: 'trade', instrument: 1, price: 6900000, qty: 70, side: 'sell' }]);
  await settle(term.page, 800);

  const after = await legend();

  assert.notDeepEqual(after, before, 'в хейкин-аши легенда не шелохнулась на живой сделке');
  assert.match(after.time, /не закрыт/, `в хейкин-аши пометки нет: «${after.time}»`);

  // Закрытие обязано быть пересчитанным по хейкину, а не сырым: иначе
  // последняя свеча жила бы по другим правилам, чем те, рядом с которыми она
  // стоит. Шаблон терпит любой разделитель разрядов, который поставит
  // toLocaleString: шаблон, знающий одну форму записи, не краснеет там, где
  // должен — на этом первая версия утверждения и прошла мутацию.
  assert.ok(!/69[^0-9]?000\.00/.test(after.ohlc), `закрытие показано сырым, а не по хейкину: «${after.ohlc}»`);

  assert.deepEqual(term.errors, [], `живые сделки не должны ронять хейкин: ${term.errors.join(' | ')}`);
});

// ---- 6. Незаведённый счёт назван, а не показан нулём ------------------------

test('счёт ещё не заведён: прочерки И названная причина, а не $0.00', async (t) => {
  // Счёт заводится первой сделкой, а не регистрацией (TRM-46). Шлюз до этого
  // отдаёт `opened: false` и `null` вместо денег (TRM-58).
  //
  // Проверяется ровно одно: что НУЛЯ НА ЭКРАНЕ НЕТ. `usd(null)` печатает
  // «$0.00» — без правки экран показал бы правдоподобный пустой счёт вместо
  // «счёта ещё нет». Это BUG-011 ещё раз: ноль — правдоподобный остаток, по
  // нему принимают решение.
  //
  // И прочерков мало: те же пять прочерков рисует обрыв связи. Поэтому
  // состояние обязано быть НАЗВАНО словами.
  //
  // Сессия нужна настоящая: без неё опрос счёта уходит раньше по ветке
  // «сессии нет», и проверялось бы не то. Первая редакция этой проверки
  // именно так и упала — показывала NO CABINET SESSION.
  const term = await withParent(t, {
    routes: {
      '/account': () => ({
        opened: false,
        balance: null,
        equity: null,
        used_margin: null,
        free_margin: null,
        open_pnl: null,
        model: null,
        negative_balance_protection: true,
      }),
    },
  });
  await term.page.evaluate(() => window.__send({ type: 'sso.token', token: 'handoff-abc' }));
  await settle(term.page, 1500);

  const frame = term.page.frames().find((f) => f.url().includes('site=apex-ru'));
  assert.ok(frame, 'терминал обязан быть во фрейме');

  const seen = await frame.evaluate(() => {
    const txt = (id) => (document.getElementById(id) || {}).textContent;
    const warn = document.getElementById('acctWarn');
    return {
      fields: {
        balance: txt('mBalance'),
        equity: txt('mEquity'),
        margin: txt('mMargin'),
        free: txt('mFree'),
        pnl: txt('mPnl'),
      },
      warnHidden: warn ? warn.hidden : null,
      warnText: warn ? warn.textContent : null,
      badgeHidden: (document.getElementById('modelBadge') || {}).hidden,
    };
  });

  for (const [name, v] of Object.entries(seen.fields)) {
    assert.notEqual(v, '$0.00', `${name}: ноль вместо «счёта нет» — это BUG-011`);
    assert.equal(v, '—', `${name}: ожидался прочерк, получено ${v}`);
  }
  assert.equal(seen.warnHidden, false, 'причина обязана быть видна: прочерков мало');
  assert.match(
    String(seen.warnText),
    // Язык по умолчанию — русский (10.11); в английском та же фраза.
    /FIRST TRADE|ПЕРВОЙ СДЕЛКОЙ/,
    `состояние обязано быть названо словами, показано: ${seen.warnText}`,
  );
  assert.equal(seen.badgeHidden, true, 'модель ещё не выбрана — значка быть не должно');
});

// ---- 10. Редизайн 10.11: вход верхним уровнем, язык, маржа, заморозка ------

test('вход верхним уровнем: токен из фрагмента обменян, фрагмент стёрт сразу', async (t) => {
  const term = await openTerminal({ query: '?site=apex-ru#handoff=handoff-top&account=real' });
  t.after(term.cleanup);
  await settle(term.page, 1500);

  const minted = term.seen.find((r) => r.path === '/v1/session' && r.method === 'POST');
  assert.ok(minted, 'терминал обязан сходить за сессией без родителя');
  assert.match(minted.body, /handoff-top/);
  const where = await term.page.evaluate(() => ({ hash: location.hash, search: location.search, acc: sessionStorage.getItem('terminal:sso.account') }));
  assert.equal(where.hash, '', 'токен не должен оставаться в адресной строке');
  assert.equal(where.search, '?site=apex-ru', 'тенант в адресе остаётся');
  assert.equal(where.acc, 'real', 'род счёта запомнен для продления');
  assert.ok(term.seen.some((r) => r.auth && r.auth.includes('session-for-handoff-top')), 'торговые ручки зовутся с сессией');

  const ui = await term.page.evaluate(() => {
    const a = /** @type {HTMLAnchorElement} */ (document.getElementById('toCabinet'));
    return { hidden: a.hidden, href: a.href, logout: document.getElementById('logoutBtn').hidden };
  });
  assert.equal(ui.hidden, false, 'верхним уровнем видна кнопка «В кабинет»');
  assert.equal(ui.href, term.origin + '/', 'адрес кабинета — из белого списка конфига');
  assert.equal(ui.logout, false, 'с сессией видна кнопка «Выйти»');
  assert.deepEqual(term.errors, []);
});

test('чужой кабинет во фрагменте не принимается: «В кабинет» ведёт только по белому списку', async (t) => {
  const term = await openTerminal({ query: '?site=apex-ru#handoff=h&account=demo&cabinet=https%3A%2F%2Fevil.example' });
  t.after(term.cleanup);
  await settle(term.page, 1000);
  const href = await term.page.evaluate(() => /** @type {HTMLAnchorElement} */ (document.getElementById('toCabinet')).href);
  assert.equal(href, term.origin + '/');
});

test('во фрейме кабинета кнопок «В кабинет»/«Выйти» нет', async (t) => {
  const term = await withParent(t);
  await term.page.evaluate(() => window.__send({ type: 'sso.token', token: 'handoff-abc' }));
  await settle(term.page, 1000);
  const frame = term.page.frames().find((f) => f.url().includes('site=apex-ru'));
  const ui = await frame.evaluate(() => ({ cab: document.getElementById('toCabinet').hidden, out: document.getElementById('logoutBtn').hidden }));
  assert.deepEqual(ui, { cab: true, out: true });
});

test('переключатель RU/EN переводит интерфейс без перезагрузки и без потери сессии', async (t) => {
  const term = await openTerminal({ query: '?site=apex-ru#handoff=handoff-lang&account=demo' });
  t.after(term.cleanup);
  await settle(term.page, 1200);
  const label = () => term.page.evaluate(() => document.querySelector('[data-i18n="acct.balance"]').textContent);
  assert.equal(await label(), 'Баланс', 'по умолчанию — русский');
  await term.page.click('.lang [data-lang="en"]');
  await settle(term.page, 600);
  assert.equal(await label(), 'Balance');
  const after = await term.page.evaluate(() => ({ lang: document.documentElement.lang, book: document.querySelector('.ob__head').textContent, token: !!window.Sso.token() }));
  assert.equal(after.lang, 'en');
  assert.match(after.book, /Market depth/);
  assert.match(after.book, /Binance/, 'источник глубины назван и по-английски');
  assert.equal(after.token, true, 'смена языка сессию не трогает');
});

test('маржин-колл: уровень ниже порога — красный баннер с уровнем и порогами', async (t) => {
  const term = await openTerminal({
    query: '?site=apex-ru#handoff=h-mc&account=demo',
    routes: {
      '/account': () => ({
        opened: true, balance: 1000000, equity: 16000, used_margin: 20000, free_margin: -4000, open_pnl: -984000,
        model: 'netting', margin_level_pct: 80, margin_call_level_pct: 100, stop_out_level_pct: 50, leverage: 50, kind: 'demo',
      }),
    },
  });
  t.after(term.cleanup);
  await settle(term.page, 1500);
  const ui = await term.page.evaluate(() => {
    const b = document.getElementById('mcBanner');
    return { hidden: b.hidden, text: b.textContent, visible: b.getBoundingClientRect().height > 0, lvl: document.getElementById('mLevel').className };
  });
  assert.equal(ui.hidden, false);
  assert.ok(ui.visible, 'баннер виден, а не только выставлен');
  assert.match(ui.text, /80%.*100%.*50%/);
  assert.match(ui.lvl, /lvl-bad/);
});

test('заморозка: бейдж, баннер, тикет закрыт плашкой, кнопки не нажимаются', async (t) => {
  const term = await openTerminal({
    query: '?site=apex-ru#handoff=h-fz&account=real',
    routes: {
      '/account': () => ({
        opened: true, balance: 1000000, equity: 1000000, used_margin: 0, free_margin: 1000000, open_pnl: 0,
        model: 'netting', leverage: 10, kind: 'real', trading_frozen: true,
      }),
    },
  });
  t.after(term.cleanup);
  await settle(term.page, 1500);
  const ui = await term.page.evaluate(() => ({
    kind: document.getElementById('kindBadge').textContent,
    badge: document.getElementById('frozenBadge').hidden,
    banner: document.getElementById('frozenBanner').hidden,
    overlay: document.getElementById('dealFrozen').getBoundingClientRect().height > 0,
    buy: /** @type {HTMLButtonElement} */ (document.getElementById('btnBuy')).disabled,
  }));
  assert.equal(ui.kind, 'REAL ACCOUNT');
  assert.deepEqual([ui.badge, ui.banner, ui.overlay, ui.buy], [false, false, true, true]);
});

test('телефон: вкладки показывают одну панель, «Ордер» открывает тикет', async (t) => {
  const term = await openTerminal({ query: '?site=apex-ru' });
  t.after(term.cleanup);
  await term.page.setViewport({ width: 390, height: 844, isMobile: true, hasTouch: true });
  await settle(term.page, 800);
  const vis = () => term.page.evaluate(() => {
    const v = (/** @type {string} */ s) => document.querySelector(s).getBoundingClientRect().height > 0;
    return { tabs: v('#mtabs'), chart: v('.center'), deal: v('#deal'), book: v('#book'), bottom: v('#bottom') };
  });
  assert.deepEqual(await vis(), { tabs: true, chart: true, deal: false, book: false, bottom: false });
  await term.page.click('#mtabs [data-m="order"]');
  await settle(term.page, 300);
  assert.deepEqual(await vis(), { tabs: true, chart: false, deal: true, book: false, bottom: false });
  const fits = await term.page.evaluate(() => document.documentElement.scrollWidth <= window.innerWidth);
  assert.ok(fits, 'страница не шире экрана телефона');
  assert.deepEqual(term.errors, []);
});

// ---- 11. Минимум стопов, график на весь экран (11.11) ----------------------

/** Счёт с минимумом стопов площадки 50 bps (0.5 %). */
const STOPS_ACCOUNT = {
  '/account': () => ({
    opened: true, balance: 1000000, equity: 1000000, used_margin: 0, free_margin: 1000000, open_pnl: 0,
    model: 'netting', leverage: 10, kind: 'demo', min_stop_bps: 50,
  }),
};

test('тикет подставляет SL/TP на минимальном расстоянии и зеркалит их для продажи', async (t) => {
  const term = await openTerminal({ query: '?site=apex-ru#handoff=h-stops&account=demo', routes: STOPS_ACCOUNT });
  t.after(term.cleanup);
  await settle(term.page, 1800);
  const read = () => term.page.evaluate(() => ({
    sl: parseFloat(/** @type {HTMLInputElement} */ (document.getElementById('sl')).value),
    tp: parseFloat(/** @type {HTMLInputElement} */ (document.getElementById('tp')).value),
    hint: document.getElementById('stopsHint').textContent,
    buyShown: document.getElementById('btnBuy').getBoundingClientRect().height > 0,
    sellShown: document.getElementById('btnSell').getBoundingClientRect().height > 0,
  }));
  // ask 65 001.00 → 0.5 % = 325.01 (вверх), у рынка + запас 10 % = 357.52.
  const buy = await read();
  assert.equal(buy.sl, 64643.48);
  assert.equal(buy.tp, 65358.52);
  assert.match(buy.hint, /0\.50%.*325\.01/, `минимум назван: «${buy.hint}»`);
  assert.deepEqual([buy.buyShown, buy.sellShown], [true, false], 'видна кнопка выбранной стороны');

  await term.page.click('#sideSell');
  await settle(term.page, 200);
  // bid 65 000.00 → 325.00 + 32.50.
  const sell = await read();
  assert.equal(sell.sl, 65357.5);
  assert.equal(sell.tp, 64642.5);
  assert.deepEqual([sell.buyShown, sell.sellShown], [false, true]);
});

test('ближе минимума тикет не отправляет, а называет ближайший допустимый уровень', async (t) => {
  const term = await openTerminal({ query: '?site=apex-ru#handoff=h-stops2&account=demo', routes: STOPS_ACCOUNT });
  t.after(term.cleanup);
  await settle(term.page, 1800);
  const sl = /** @type {any} */ (await term.page.$('#sl'));
  await sl.click({ clickCount: 3 });
  await sl.type('64900');
  await settle(term.page, 100);
  const before = term.seen.filter((r) => r.path === '/deals' && r.method === 'POST').length;
  await term.page.click('#btnBuy');
  await settle(term.page, 400);
  const ui = await term.page.evaluate(() => ({
    msg: document.getElementById('dealMsg').textContent,
    bad: document.getElementById('sl').classList.contains('bad'),
    kept: /** @type {HTMLInputElement} */ (document.getElementById('sl')).value,
  }));
  assert.equal(term.seen.filter((r) => r.path === '/deals' && r.method === 'POST').length, before, 'запрос не ушёл');
  assert.ok(ui.bad, 'поле подсвечено');
  assert.match(ui.msg, /Stop Loss.*64,675\.99/, `причина и уровень: «${ui.msg}»`);
  assert.equal(ui.kept, '64900', 'тронутое человеком поле тикет не переписывает');
});

test('отказ шлюза «стоп ближе минимума» показан словами и с уровнем шлюза', async (t) => {
  const term = await openTerminal({
    query: '?site=apex-ru#handoff=h-stops3&account=demo',
    routes: Object.assign({}, STOPS_ACCOUNT, {
      '/deals': (_u, req) => (req.method === 'POST'
        ? { __status: 422, body: { error: 'take_profit_too_close', limit: 6540000, min_stop_bps: 50, min_distance: 32501, reference: 6500100 } }
        : []),
    }),
  });
  t.after(term.cleanup);
  await settle(term.page, 1800);
  await term.page.click('#btnBuy');
  await settle(term.page, 500);
  const msg = await term.page.evaluate(() => document.getElementById('dealMsg').textContent);
  assert.match(msg, /Take Profit/);
  assert.doesNotMatch(msg, /take_profit_too_close/, 'не сырым кодом');
  assert.match(msg, /65,400\.00/, `ближайший уровень от шлюза: «${msg}»`);
});

/** Размер графика: секция, область и холст библиотеки. */
const chartBox = (/** @type {any} */ ctx) => ctx.evaluate(() => {
  const sec = document.querySelector('.center').getBoundingClientRect();
  const c = /** @type {HTMLCanvasElement} */ (document.querySelector('#chart canvas'));
  return {
    fs: document.fullscreenElement === document.querySelector('.center'),
    max: document.querySelector('.center').classList.contains('is-max'),
    w: Math.round(sec.width), h: Math.round(sec.height),
    canvasW: c ? c.clientWidth : 0,
    toolbar: document.querySelector('.toolbar').getBoundingClientRect().height > 0,
    vw: innerWidth, vh: innerHeight,
  };
});

test('график на весь экран: Fullscreen API, тулбар остаётся, график перерисован, выход кнопкой', async (t) => {
  const term = await openTerminal({ query: '?site=apex-ru' });
  t.after(term.cleanup);
  await settle(term.page, 1500);
  const before = await chartBox(term.page);
  await term.page.click('#fullBtn');
  await settle(term.page, 600);
  const on = await chartBox(term.page);
  assert.equal(on.fs, true, 'во весь экран уходит вся секция графика');
  assert.ok(on.toolbar, 'тулбар с кнопкой выхода на экране');
  assert.ok(on.canvasW > before.canvasW + 200, `график перерисован под новый размер: ${before.canvasW} → ${on.canvasW}`);
  await term.page.click('#fullBtn');
  await settle(term.page, 600);
  const off = await chartBox(term.page);
  assert.equal(off.fs, false);
  assert.ok(Math.abs(off.canvasW - before.canvasW) < 4, 'размер вернулся');
  assert.deepEqual(term.errors, []);
});

test('во фрейме без allow="fullscreen" график разворачивается на окно, Esc и кнопка возвращают', async (t) => {
  // Ровно так терминал живёт в кабинете: фрейм с другого origin, API закрыт.
  // Прежде кнопка здесь не делала ничего — отказ глотался молча.
  const term = await withParent(t, { foreign: true });
  const frame = term.page.frames().find((f) => f.url().includes('site=apex-ru'));
  assert.equal(await frame.evaluate(() => document.fullscreenEnabled), false, 'предпосылка: API во фрейме закрыт');
  const before = await chartBox(frame);
  await frame.click('#fullBtn');
  await settle(term.page, 600);
  const on = await chartBox(frame);
  assert.equal(on.max, true, 'запасной режим включён');
  assert.deepEqual([on.w, on.h], [on.vw, on.vh], 'секция графика — на всё окно терминала');
  assert.ok(on.canvasW > before.canvasW + 200, `график перерисован: ${before.canvasW} → ${on.canvasW}`);
  await frame.focus('#fullBtn');
  await term.page.keyboard.press('Escape');
  await settle(term.page, 500);
  assert.equal((await chartBox(frame)).max, false, 'Esc возвращает раскладку');
  await frame.click('#fullBtn');
  await settle(term.page, 300);
  await frame.click('#fullBtn');
  await settle(term.page, 500);
  const off = await chartBox(frame);
  assert.equal(off.max, false, 'кнопка тоже возвращает');
  assert.ok(Math.abs(off.canvasW - before.canvasW) < 4);
});
