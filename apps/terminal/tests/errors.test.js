'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadWidget } = require('./helpers/load-widget');

function load(t) {
  const env = loadWidget(['errors.js'], {});
  t.after(env.cleanup);
  return /** @type {any} */ (env.window);
}

test('ошибка при загрузке видна на экране: текст, файл и строка, браузер', (t) => {
  const w = load(t);
  w.dispatchEvent(new w.ErrorEvent('error', { message: 'TypeError: x is not a function', filename: 'https://t.example/app.js?v=1', lineno: 42, colno: 7 }));
  const box = w.document.getElementById('fatalErrors');
  assert.ok(box, 'плашка появилась');
  assert.match(box.textContent, /TypeError: x is not a function \(app\.js:42:7\)/);
  assert.match(box.textContent, /скриншот/);
  assert.match(box.textContent, new RegExp(w.navigator.userAgent.slice(0, 20).replace(/[.*+?^${}()|[\]\\]/g, '\\$&')));
});

test('одинаковые ошибки не дублируются, больше пяти не копится', (t) => {
  const w = load(t);
  for (let i = 0; i < 3; i++) w.dispatchEvent(new w.ErrorEvent('error', { message: 'same' }));
  for (let i = 0; i < 10; i++) w.dispatchEvent(new w.ErrorEvent('error', { message: 'e' + i }));
  assert.equal(w.TerminalErrors.shown().length, 5);
  assert.equal(w.TerminalErrors.shown().filter((/** @type {string} */ s) => s === 'same').length, 1);
});

test('ошибка позже 30 секунд после загрузки плашку не показывает', (t) => {
  const w = load(t);
  const realNow = w.Date.now;
  w.Date.now = () => realNow() + 31000;
  w.dispatchEvent(new w.ErrorEvent('error', { message: 'late' }));
  assert.equal(w.document.getElementById('fatalErrors'), null);
});
