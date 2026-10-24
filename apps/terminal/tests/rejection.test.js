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
