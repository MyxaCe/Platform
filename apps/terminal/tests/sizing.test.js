'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadModule } = require('./helpers/load-module');

const Sizing = loadModule('sizing.js', 'Sizing');
const num = (v, d) => v.toFixed(d);

const EURUSD = { qty_decimals: 0, qty_step: 1000, min_qty: 1000, contract_size: '100000', lot_decimals: 2 };
const XAUUSD = { qty_decimals: 2, qty_step: 100, min_qty: 100, contract_size: '100', lot_decimals: 2 };
const BTC = { qty_decimals: 5 };

test('форекс: 0.01 лота = 1000 единиц и обратно', () => {
  assert.equal(Sizing.contract(EURUSD), 100000);
  assert.equal(Sizing.fromLots(EURUSD, 0.01), 1000);
  assert.equal(Sizing.fromLots(EURUSD, 1), 100000);
  assert.equal(Sizing.fromLots(EURUSD, 0.07), 7000);
  assert.equal(Sizing.fromLots(EURUSD, 0.29), 29000);
  assert.equal(Sizing.toLots(EURUSD, 1000), 0.01);
  assert.equal(Sizing.format(EURUSD, 25000, num), '0.25');
});

test('объём округляется вниз до шага, а не вверх', () => {
  assert.equal(Sizing.fromLots(EURUSD, 0.019), 1000);
  assert.equal(Sizing.fromLots(EURUSD, 0.005), 0);
  assert.equal(Sizing.fromUnits(XAUUSD, 1.99), 100);
});

test('ниже минимума — видно; на минимуме — нет', () => {
  assert.equal(Sizing.belowMin(EURUSD, 0), true);
  assert.equal(Sizing.belowMin(EURUSD, 999), true);
  assert.equal(Sizing.belowMin(EURUSD, 1000), false);
  assert.equal(Sizing.belowMin(BTC, 1), false, 'без min_qty ограничения нет');
});

test('металл: контракт 100 унций, точность лота из lot_decimals', () => {
  assert.equal(Sizing.fromLots(XAUUSD, 0.01), 100);
  assert.equal(Sizing.toLots(XAUUSD, 100), 0.01);
  assert.equal(Sizing.lotDecimals(XAUUSD), 2);
  assert.equal(Sizing.lotDecimals(Object.assign({}, XAUUSD, { lot_decimals: null })), 2, 'без lot_decimals — из шага');
});

test('крипто без contract_size: перевод тот же, что был в тикете', () => {
  assert.equal(Sizing.contract(BTC), null);
  assert.equal(Sizing.contract({ contract_size: '1' }), null);
  assert.equal(Sizing.contract({ contract_size: null }), null);
  for (const v of [0.01, 0.001, 0.12345, 1.5, 0.000123456, 3.33333333]) {
    assert.equal(Sizing.fromUnits(BTC, v), Math.round(v * 1e5), `объём ${v}`);
    assert.equal(Sizing.fromLots(BTC, v), Math.round(v * 1e5), `лот = единица у крипто: ${v}`);
  }
  assert.equal(Sizing.format(BTC, 1500, num), '0.01500');
  assert.equal(Sizing.fromUnits(undefined, 0.01), 10, 'без инструмента — прежняя точность 3');
});

test('ноль, отрицательный и нечисловой объём не превращаются в заявку', () => {
  assert.equal(Sizing.fromUnits(EURUSD, 0), 0);
  assert.equal(Sizing.fromUnits(EURUSD, -1), 0);
  assert.equal(Sizing.fromUnits(EURUSD, NaN), 0);
  assert.equal(Sizing.fromUnits(EURUSD, Infinity), 0);
});
