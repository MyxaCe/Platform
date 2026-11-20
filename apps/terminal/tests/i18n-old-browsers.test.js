'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadWidget } = require('./helpers/load-widget');

function load(t) {
  const env = loadWidget(['rejection.js', 'i18n.js'], {});
  t.after(env.cleanup);
  return /** @type {any} */ (env.window);
}

test('браузер без narrowSymbol: деньги форматируются, терминал не падает', (t) => {
  const w = load(t);
  const Real = w.Intl.NumberFormat;
  w.Intl.NumberFormat = function (/** @type {any} */ tag, /** @type {any} */ opts) {
    if (opts && opts.currencyDisplay === 'narrowSymbol') throw new RangeError('Invalid currencyDisplay');
    return new Real(tag, opts);
  };
  const s = w.I18n.money(123456);
  assert.match(s, /1.?234,56|1,234\.56/);
  assert.match(s, /\$|US\$|USD/);
});

test('браузер, где Intl.NumberFormat не работает совсем: числа, деньги и проценты всё равно выводятся', (t) => {
  const w = load(t);
  w.Intl.NumberFormat = function () { throw new RangeError('broken'); };
  assert.equal(w.I18n.money(123456), '$1234.56');
  assert.equal(w.I18n.money(-500), '-$5.00');
  assert.equal(w.I18n.num(3.14159, 2), '3.14');
  assert.equal(w.I18n.pct(12.5, 1), '12.5%');
  assert.equal(w.I18n.change(-0.42), '-0.42%');
  assert.equal(w.I18n.change(1.5), '+1.50%');
});
