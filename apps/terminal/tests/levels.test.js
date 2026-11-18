'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');
const { loadModule } = require('./helpers/load-module');

const Stops = loadModule('stops.js', 'Stops');
function loadLevels() {
  const code = fs.readFileSync(path.join(__dirname, '..', 'public', 'levels.js'), 'utf8');
  const sandbox = { window: { Stops }, document: undefined, console };
  vm.createContext(sandbox);
  vm.runInContext(code, sandbox, { filename: 'levels.js' });
  return sandbox.window.Levels;
}
const Levels = loadLevels();
const plain = (/** @type {any} */ o) => JSON.parse(JSON.stringify(o));

test('поле: пусто — уровня нет, мусор и ноль — ошибка ввода, запятая — как точка', () => {
  assert.equal(Levels.parse('', 2), null);
  assert.equal(Levels.parse('  ', 2), null);
  assert.equal(Levels.parse('64900', 2), 6_490_000);
  assert.equal(Levels.parse('64900,5', 2), 6_490_050);
  assert.ok(Number.isNaN(Levels.parse('0', 2)));
  assert.ok(Number.isNaN(Levels.parse('-1', 2)));
  assert.ok(Number.isNaN(Levels.parse('abc', 2)));
});

test('тело: только изменённые поля, `null` — снять, ничего не изменено — null', () => {
  const cur = { sl: 100, tp: null };
  assert.deepEqual(plain(Levels.body(cur, { sl: 90, tp: null })), { sl: 90 });
  assert.deepEqual(plain(Levels.body(cur, { sl: null, tp: null })), { sl: null });
  assert.deepEqual(plain(Levels.body(cur, { sl: 100, tp: 200 })), { tp: 200 });
  assert.equal(Levels.body(cur, { sl: 100, tp: null }), null);
});

test('минимум площадки: как на шлюзе, от цены отсчёта, только у изменённого уровня', () => {
  const ref = 4_990_000;
  const cur = { sl: null, tp: null };
  const near = Levels.evaluate({ side: 'buy', ref, cur, next: { sl: ref - 24_949, tp: null }, bps: 50 });
  assert.deepEqual(plain(near.sl), { code: 'stop_loss_too_close', limit: ref - 24_950 });
  assert.equal(Levels.evaluate({ side: 'buy', ref, cur, next: { sl: ref - 24_950, tp: null }, bps: 50 }).sl, null, 'ровно на границе — можно');
  const s = Levels.evaluate({ side: 'sell', ref, cur, next: { sl: null, tp: ref - 100 }, bps: 50 });
  assert.equal(s.tp.code, 'take_profit_too_close');
  const kept = Levels.evaluate({ side: 'buy', ref, cur: { sl: ref - 10, tp: null }, next: { sl: ref - 10, tp: ref + 50_000 }, bps: 50 });
  assert.deepEqual(plain(kept), { sl: null, tp: null });
  assert.deepEqual(plain(Levels.evaluate({ side: 'buy', ref, cur: { sl: 1, tp: 2 }, next: { sl: null, tp: null }, bps: 50 })), { sl: null, tp: null });
});

test('уровень по ту сторону цены — «сработал бы сразу», и при нулевом минимуме тоже', () => {
  const ref = 5_000_000;
  const cur = { sl: null, tp: null };
  assert.equal(Levels.evaluate({ side: 'buy', ref, cur, next: { sl: ref + 50, tp: null }, bps: 0 }).sl.code, 'stop_loss_already_reached');
  assert.equal(Levels.evaluate({ side: 'buy', ref, cur, next: { sl: null, tp: ref - 1 }, bps: 0 }).tp.code, 'take_profit_already_reached');
  assert.equal(Levels.evaluate({ side: 'sell', ref, cur, next: { sl: ref - 1, tp: null }, bps: 0 }).sl.code, 'stop_loss_already_reached');
  assert.equal(Levels.evaluate({ side: 'buy', ref, cur, next: { sl: NaN, tp: null }, bps: 0 }).sl.code, 'invalid_level');
  assert.deepEqual(plain(Levels.evaluate({ side: 'buy', ref: null, cur, next: { sl: ref + 50, tp: null }, bps: 50 })), { sl: null, tp: null });
});
