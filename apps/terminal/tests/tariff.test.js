'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadModule } = require('./helpers/load-module');

const Tariff = loadModule('tariff.js', 'Tariff');
const PX = 5_000_000;

test('наценка: покупка дороже, продажа дешевле, ноль — рынок', () => {
  assert.equal(Tariff.execPrice(true, PX, 10), PX + 5000);
  assert.equal(Tariff.execPrice(false, PX, 10), PX - 5000);
  assert.equal(Tariff.execPrice(true, PX, 0), PX);
  assert.equal(Tariff.execPrice(true, PX, undefined), PX, 'тарифа не прислали — рынок');
  assert.equal(Tariff.execPrice(true, 5000, 1), 5001);
  assert.equal(Tariff.execPrice(true, 15, 1), 15);
});

test('комиссия — как в ядре, с округлением к ближайшему', () => {
  assert.equal(Tariff.commissionMinor(5_005_000, 1000, 2, 3, 7), 3504);
  assert.equal(Tariff.commissionMinor(PX, 1000, 2, 3, 0), 0);
  assert.equal(Tariff.commissionMinor(PX, 1000, 2, 5, 10), 50);
});

test('строка тикета: есть комиссия — сумма; нет тарифа — строки нет', () => {
  assert.equal(Tariff.commissionText(50, 10), 'Commission ≈ 0.50 USD');
  assert.equal(Tariff.commissionText(0, 0), '');
  assert.equal(Tariff.commissionText(NaN, 10), 'Commission ≈ — USD', 'цены нет — прочерк, не ноль');
});

test('плечо сделки — по категории инструмента, без таблицы — плечо счёта', () => {
  const acc = { leverage: 20, leverage_classes: { crypto: 5, forex: 100 } };
  assert.equal(Tariff.leverageFor(acc, { category: 'forex' }), 100);
  assert.equal(Tariff.leverageFor(acc, { category: 'crypto' }), 5);
  assert.equal(Tariff.leverageFor(acc, { category: 'metals' }), 20, 'категории нет в таблице — базовое');
  assert.equal(Tariff.leverageFor(acc, {}), 20);
  assert.equal(Tariff.leverageFor({ leverage: 20 }, { category: 'forex' }), 20, 'старый шлюз без таблицы');
  assert.equal(Tariff.leverageFor({ leverage: 20, leverage_classes: { forex: 0 } }, { category: 'forex' }), 20);
  assert.equal(Tariff.leverageFor(null, { category: 'forex' }), null);
  assert.equal(Tariff.leverageFor({ leverage: 0 }, { category: 'forex' }), null);
});
