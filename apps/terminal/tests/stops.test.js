'use strict';
// ============================================================================
// Минимальное расстояние SL/TP в тикете. Числа — те же, что в тестах шлюза
// (`stops::unit`, `stops_closer_than_the_minimum_are_refused_by_the_gateway`):
// одна арифметика на две стороны границы.
// ============================================================================
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadModule } = require('./helpers/load-module');

const Stops = loadModule('stops.js', 'Stops');
/** Объект из песочницы vm — с чужим прототипом; сравниваем по содержимому. */
const plain = (/** @type {any} */ o) => JSON.parse(JSON.stringify(o));

test('расстояние: вверх, ноль — ограничения нет', () => {
  assert.equal(Stops.minDistance(6_500_000, 50), 32_500);
  assert.equal(Stops.minDistance(101, 1), 1);
  assert.equal(Stops.minDistance(6_500_000, 0), 0);
  assert.equal(Stops.minDistance(0, 50), 0);
});

test('проверка зеркальна для покупки и продажи — как на шлюзе', () => {
  const px = 4_800_000; // 1 % = 48 000
  assert.deepEqual(plain(Stops.check('buy', px, 4_760_000, null, 100)), { code: 'stop_loss_too_close', limit: 4_752_000, distance: 48_000 });
  assert.equal(Stops.check('buy', px, null, 4_840_000, 100).code, 'take_profit_too_close');
  assert.equal(Stops.check('sell', px, 4_840_000, null, 100).limit, 4_848_000);
  assert.equal(Stops.check('sell', px, null, 4_760_000, 100).code, 'take_profit_too_close');
  assert.equal(Stops.check('buy', px, 4_752_000, 4_848_000, 100), null, 'ровно на минимуме — можно');
  assert.equal(Stops.check('buy', px, null, null, 100), null, 'без уровней — можно');
  assert.equal(Stops.check('buy', px, 4_799_999, 4_800_001, 0), null, 'ноль — без ограничения');
});

test('подстановка: на минимуме, у рынка — с запасом, и всегда проходит проверку', () => {
  const px = 4_800_000;
  const lim = Stops.suggest('buy', px, 100, false);
  assert.deepEqual(plain(lim), { sl: 4_752_000, tp: 4_848_000, distance: 48_000, hint: false });
  const mkt = Stops.suggest('sell', px, 100, true);
  assert.equal(mkt.sl, px + 48_000 + 4_800);
  assert.equal(mkt.tp, px - 48_000 - 4_800);
  for (const side of ['buy', 'sell']) {
    for (const market of [true, false]) {
      const s = Stops.suggest(side, px, 37, market);
      assert.equal(Stops.check(side, px, s.sl, s.tp, 37), null, `${side}/${market}`);
    }
  }
});

test('минимума нет — подсказка 1 %, и она так и называется', () => {
  const s = Stops.suggest('buy', 6_500_000, 0, true);
  assert.deepEqual(plain(s), { sl: 6_435_000, tp: 6_565_000, distance: 0, hint: true });
  assert.equal(Stops.suggest('buy', null, 50, true), null, 'цены нет — подставлять не от чего');
});
