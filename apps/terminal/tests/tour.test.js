'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadWidget, click } = require('./helpers/load-widget');

const CABINET = 'https://cabinet.example.test';
/** Объекты из окна jsdom — другого «мира»: сравниваем по содержимому. */
const plain = (v) => JSON.parse(JSON.stringify(v));
const labels = { back: 'Назад', next: 'Далее', skip: 'Пропустить', done: 'Готово', help: 'Экскурсия', stepOf: 'Шаг {current} из {total}' };
const config = (over = {}) => ({
  type: 'onboarding.tour.config',
  version: 2,
  labels,
  autostart: false,
  steps: [
    { anchor: 'terminal.pairs', title: 'Инструменты', body: 'Список' },
    { anchor: 'terminal.close', title: 'Закрыть', body: 'Нет позиций — шаг пропускается' },
    { anchor: 'terminal.sltp', title: 'Стопы', body: 'SL и TP' },
  ],
  ...over,
});

/** Терминал во «фрейме»: разметка с якорями, видимые прямоугольники, parent со своим postMessage. */
function terminal(t) {
  const env = loadWidget([]);
  t.after(env.cleanup);
  const w = /** @type {any} */ (env.window);
  env.document.body.innerHTML = `
    <header><div class="top__right"><button id="themeToggle">◐</button></div></header>
    <nav id="mtabs"><button data-m="chart"></button><button data-m="order"></button><button data-m="positions"></button></nav>
    <div id="pairs" class="pairs">pairs</div>
    <div class="fld fld--sl">sl</div><div class="fld fld--tp">tp</div>`;
  const rects = { pairs: [10, 10, 200, 300], 'fld--sl': [400, 500, 100, 30], 'fld--tp': [400, 540, 100, 30] };
  w.HTMLElement.prototype.getClientRects = function () { return this.isConnected ? [1] : []; };
  w.HTMLElement.prototype.getBoundingClientRect = function () {
    const key = this.id || [...this.classList].find((c) => rects[c]) || '';
    const [top, left, width, height] = rects[key] || [0, 0, 50, 20];
    return { top, left, width, height, right: left + width, bottom: top + height, x: left, y: top, toJSON() {} };
  };
  w.HTMLElement.prototype.scrollIntoView = function () {};
  w.requestAnimationFrame = (cb) => { cb(0); return 1; };
  w.innerWidth = 1280; w.innerHeight = 800;
  const sent = [];
  const parent = { postMessage: (data, origin) => sent.push({ data, origin }) };
  Object.defineProperty(w, 'parent', { value: parent, configurable: true });
  const evaluate = (code) => w.eval(code);
  evaluate(require('node:fs').readFileSync(require('node:path').join(__dirname, '..', 'public', 'tour.js'), 'utf8'));
  const deliver = (data, origin = CABINET) => w.dispatchEvent(new w.MessageEvent('message', { data, origin }));
  return { w, doc: env.document, sent, deliver };
}

test('сообщение кабинета: чужие элементы, не-строки и лишнее отбрасываются; без шагов — нет экскурсии', (t) => {
  const { w } = terminal(t);
  const parsed = w.Tour.parseConfig(config({ steps: [
    { anchor: 'terminal.chart', title: '<b>График</b>', body: 'x' },
    { anchor: '#pairs', title: 'селектор нельзя', body: 'x' },
    { anchor: 'cabinet.trade', title: 'чужая часть', body: 'x' },
    { anchor: 'terminal.deal', title: 42, body: null },
  ] }));
  assert.deepEqual(plain(parsed.steps), [{ anchor: 'terminal.chart', title: '<b>График</b>', body: 'x' }]);
  assert.equal(w.Tour.parseConfig(config({ steps: [] })), null);
  assert.equal(w.Tour.parseConfig(config({ version: 0 })), null);
  assert.equal(w.Tour.parseConfig({ type: 'sso.token' }), null);
});

test('без кабинета (нет разрешённых источников) экскурсия не подключается', (t) => {
  const { w, doc, sent } = terminal(t);
  w.Tour.init({ cabinetOrigins: [] });
  assert.equal(sent.length, 0);
  assert.equal(doc.getElementById('tourHelp'), null);
});

test('кабинет: ready → config; чужой источник игнорируется; кнопка «?» появляется с подписью из CMS', (t) => {
  const { w, doc, sent, deliver } = terminal(t);
  w.Tour.init({ cabinetOrigins: [CABINET] });
  assert.deepEqual(plain(sent), [{ data: { type: 'onboarding.tour.ready' }, origin: CABINET }]);
  deliver(config(), 'https://evil.example.test');
  assert.equal(doc.getElementById('tourHelp'), null);
  deliver(config());
  const btn = doc.getElementById('tourHelp');
  assert.ok(btn);
  assert.equal(btn.getAttribute('aria-label'), 'Экскурсия');
  assert.equal(btn.nextElementSibling.id, 'themeToggle');
});

test('экскурсия: шаг без элемента пропускается, текст — как текст (без HTML), «Готово» возвращает результат кабинету', (t) => {
  const { w, doc, sent, deliver } = terminal(t);
  w.Tour.init({ cabinetOrigins: [CABINET] });
  deliver(config({ steps: [
    { anchor: 'terminal.pairs', title: '<img src=x onerror=alert(1)>', body: 'Список' },
    { anchor: 'terminal.close', title: 'Закрыть', body: 'нет позиций' },
    { anchor: 'terminal.sltp', title: 'Стопы', body: 'SL и TP' },
  ] }));
  click(w, doc.getElementById('tourHelp'));
  const dialog = doc.querySelector('[role="dialog"]');
  assert.ok(dialog);
  assert.equal(doc.querySelector('.tour__title').textContent, '<img src=x onerror=alert(1)>');
  assert.equal(doc.querySelector('.tour__title img'), null);
  assert.equal(doc.querySelector('.tour__count').textContent, 'Шаг 1 из 3');
  click(w, doc.querySelector('.tour__next'));
  assert.equal(doc.querySelector('.tour__title').textContent, 'Стопы');
  const hl = /** @type {HTMLElement} */ (doc.querySelector('.tour__hl'));
  assert.equal(hl.style.top, '394px', 'подсветка охватывает и SL, и TP');
  assert.equal(hl.style.width, '152px');
  assert.equal(doc.querySelector('.tour__next').textContent, 'Готово');
  click(w, doc.querySelector('.tour__next'));
  assert.equal(doc.querySelector('.tour'), null);
  assert.deepEqual(plain(sent.at(-1)), { data: { type: 'onboarding.tour.result', version: 2, status: 'done' }, origin: CABINET });
});

test('Esc — пропустить; автозапуск только по флагу из кабинета', async (t) => {
  const { w, doc, sent, deliver } = terminal(t);
  w.Tour.init({ cabinetOrigins: [CABINET] });
  deliver(config({ autostart: true }));
  await new Promise((r) => setTimeout(r, 900));
  assert.ok(doc.querySelector('.tour'));
  w.dispatchEvent(new w.KeyboardEvent('keydown', { key: 'Escape' }));
  assert.equal(doc.querySelector('.tour'), null);
  assert.equal(sent.at(-1).data.status, 'skipped');
});

test('нет ни одного элемента на экране — экскурсия не открывается и результат не отправляется', (t) => {
  const { w, doc, sent, deliver } = terminal(t);
  w.Tour.init({ cabinetOrigins: [CABINET] });
  deliver(config({ steps: [{ anchor: 'terminal.close', title: 'Закрыть', body: 'x' }] }));
  click(w, doc.getElementById('tourHelp'));
  assert.equal(doc.querySelector('.tour'), null);
  assert.equal(sent.length, 1, 'только ready');
});

test('карточка: снизу элемента, если влезает, иначе сверху; узкий экран — нижняя панель', (t) => {
  const { w } = terminal(t);
  assert.deepEqual(plain(w.Tour.placeCard({ top: 100, left: 10, width: 50, height: 40 }, { width: 1280, height: 800 }, 180)), { mode: 'float', top: 158, left: 12 });
  assert.deepEqual(plain(w.Tour.placeCard({ top: 650, left: 10, width: 50, height: 40 }, { width: 1280, height: 800 }, 180)), { mode: 'float', top: 452, left: 12 });
  assert.deepEqual(plain(w.Tour.placeCard({ top: 100, left: 10, width: 50, height: 40 }, { width: 400, height: 800 }, 180)), { mode: 'sheet' });
});
