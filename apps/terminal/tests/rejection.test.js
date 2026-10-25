'use strict';
// ============================================================================
// Отказ границы объясняется словами (TRM-43, КОНТРАКТЫ §4б).
//
// Проверяется не «функция вернула строку», а то, ради чего она существует:
// **разные причины ведут к разным действиям разных людей**, и человек по тексту
// должен понимать, к кому идти. Пока таблица жила внутри `app.js`, проверить это
// было нечем — и недостающую строку нашёл пользователь: `503
// tenant_instruments_unreadable` показывался сырым кодом и 24.10 отправил
// наугад команду развёртывания.
// ============================================================================
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadModule } = require('./helpers/load-module');

const Rejection = loadModule('rejection.js', 'Rejection');

/** Коды отказа границы, которые шлюз действительно отдаёт (gateway/src/lib.rs). */
const GATE_CODES = [
  'instrument_not_allowed',
  'tenant_instruments_unavailable',
  'tenant_instruments_unreadable',
];

test('у каждого отказа границы есть объяснение, а не код в лицо', () => {
  for (const code of GATE_CODES) {
    const t = Rejection.text(403, code);
    assert.ok(
      !t.includes(code),
      `«${code}» показывается человеку сырым кодом: ${t}`,
    );
    assert.ok(t.length > 20, `объяснение «${t}» слишком короткое, чтобы что-то объяснить`);
  }
});

test('три причины — три РАЗНЫХ текста, а не один экран', () => {
  const texts = GATE_CODES.map((c) => Rejection.text(503, c));
  assert.equal(
    new Set(texts).size,
    GATE_CODES.length,
    `одинаковые тексты отправляют чинить наугад: ${JSON.stringify(texts)}`,
  );
});

// Главный случай задачи. Оба кода — 503 и оба «торговля закрыта», поэтому их и
// склеивали. Но чинят их РАЗНЫЕ люди, и текст обязан это различать: один про
// недоступный источник, другой про содержимое списка.
test('«списка нет» и «список непрочитаем» зовут разных людей', () => {
  const unavailable = Rejection.text(503, 'tenant_instruments_unavailable');
  const unreadable = Rejection.text(503, 'tenant_instruments_unreadable');

  assert.match(
    unavailable,
    /unreachable|did not arrive/i,
    `«списка нет» обязан говорить о недоступности источника: ${unavailable}`,
  );
  assert.match(
    unreadable,
    /CMS/,
    `«список непрочитаем» обязан звать владельца в CMS: ${unreadable}`,
  );
  assert.ok(
    !/CMS/.test(unavailable),
    `«списка нет» не должен звать в CMS — там чинить нечего: ${unavailable}`,
  );
});

// Запасная ветка называет себя запасной. Фраза на человеческом языке,
// выдуманная под незнакомый код, была бы отказом, похожим на объяснение.
test('неизвестный код не притворяется объяснением', () => {
  const t = Rejection.text(422, 'invariant_violated');
  assert.match(t, /does not recognise/i, `запасная ветка обязана признаться: ${t}`);
  assert.ok(t.includes('invariant_violated'), 'и назвать код, чтобы было с чем идти');
  assert.ok(t.includes('422'), 'и статус');
});

test('отсутствие сессии объясняется даже без кода', () => {
  assert.match(Rejection.text(401, undefined), /session/i);
});

// Код точнее статуса. Ветка по статусу стоит НИЖЕ таблицы намеренно: иначе
// `401` затёр бы точную причину, если шлюз однажды начнёт её присылать.
test('код не затирается статусом', () => {
  assert.equal(
    Rejection.text(401, 'instrument_not_allowed'),
    Rejection.TEXT.instrument_not_allowed,
  );
});

// ---- Уровень, уже достигнутый рынком (TRM-57) -------------------------------

/** Коды отказа по уровню, которые шлюз отдаёт на `/deals` и `/pending`. */
const LEVEL_CODES = ['stop_loss_already_reached', 'take_profit_already_reached'];

test('уровень, уже достигнутый рынком, объяснён, а не показан кодом', () => {
  for (const code of LEVEL_CODES) {
    const t = Rejection.text(409, code);
    assert.ok(!t.includes(code), `«${code}» показывается сырым кодом: ${t}`);
    assert.ok(
      !/terminal does not know/i.test(t),
      `«${code}» уехал в запасную ветку — объяснения нет: ${t}`,
    );
  }
});

test('стоп и тейк — РАЗНЫЕ тексты: человек правит разные поля', () => {
  const [sl, tp] = LEVEL_CODES.map((c) => Rejection.text(409, c));
  assert.notEqual(sl, tp, 'один текст на оба случая отправит проверять оба поля');
  assert.match(sl, /stop loss/i, `текст обязан назвать поле: ${sl}`);
  assert.match(tp, /take profit/i, `текст обязан назвать поле: ${tp}`);
});

test('текст говорит, ЧТО сделать, а не только что отказано', () => {
  // Отказ без действия — это сообщение «нельзя», после которого человек
  // пробует то же самое ещё раз. Здесь действие есть: сдвинуть уровень, и
  // названа сторона — для покупки и для продажи она разная.
  for (const code of LEVEL_CODES) {
    const t = Rejection.text(409, code);
    assert.match(t, /move it/i, `нет действия: ${t}`);
    assert.match(t, /buy/i, `не названа сторона для покупки: ${t}`);
    assert.match(t, /sell/i, `не названа сторона для продажи: ${t}`);
  }
});

// ---- Заведение счёта: сумму называет сайт (Р-293) ---------------------------

/** Коды, которыми шлюз отказывает в ЗАВЕДЕНИИ счёта. */
const OPEN_CODES = [
  'demo_start_balance_not_set',
  'demo_start_balance_unavailable',
  'identity_store_unavailable',
];

test('отказ в заведении счёта объяснён, а не показан кодом', () => {
  for (const code of OPEN_CODES) {
    const t = Rejection.text(409, code);
    assert.ok(!t.includes(code), `«${code}» показывается сырым кодом: ${t}`);
    assert.ok(
      !/terminal does not know/i.test(t),
      `«${code}» уехал в запасную ветку: ${t}`,
    );
  }
});

test('три причины — три РАЗНЫХ текста: чинят их разные люди', () => {
  const texts = OPEN_CODES.map((c) => Rejection.text(409, c));
  assert.equal(new Set(texts).size, texts.length, `тексты совпадают: ${texts.join(' | ')}`);
  // «Сумма не названа» чинит ВЛАДЕЛЕЦ в админке, и текст обязан это сказать:
  // иначе человек напишет в поддержку, а чинится это правкой настройки.
  assert.match(Rejection.text(409, 'demo_start_balance_not_set'), /admin panel/i);
});

test('отказ не обвиняет человека в его же запросе', () => {
  // Все три — не про то, что он сделал. Если текст не скажет этого прямо,
  // человек будет править заявку, в которой нечего править.
  for (const code of OPEN_CODES) {
    assert.match(
      Rejection.text(409, code),
      /nothing is wrong with your request|your request was fine/i,
      `${code}: человек подумает, что виноват он: ${Rejection.text(409, code)}`,
    );
  }
});
