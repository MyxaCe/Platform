'use strict';
// ============================================================================
// Тариф в тикете ордера (решение владельца 10.11).
//
// Тикет обязан показывать ТУ ЖЕ цену и ТУ ЖЕ комиссию, что исполнит шлюз:
// числа ниже — из тестов брокера (`fees_tests::helpers_round_to_nearest`),
// одна арифметика на две стороны границы.
// ============================================================================
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadModule } = require('./helpers/load-module');

const Tariff = loadModule('tariff.js', 'Tariff');
const PX = 5_000_000; // 50 000.00 при pd=2

test('наценка: покупка дороже, продажа дешевле, ноль — рынок', () => {
  assert.equal(Tariff.execPrice(true, PX, 10), PX + 5000);
  assert.equal(Tariff.execPrice(false, PX, 10), PX - 5000);
  assert.equal(Tariff.execPrice(true, PX, 0), PX);
  assert.equal(Tariff.execPrice(true, PX, undefined), PX, 'тарифа не прислали — рынок');
  // Половина — от нуля, как в ядре: 1 bps от 5000 = 0.5 → 1.
  assert.equal(Tariff.execPrice(true, 5000, 1), 5001);
  assert.equal(Tariff.execPrice(true, 15, 1), 15);
});

test('комиссия — как в ядре, с округлением к ближайшему', () => {
  // 50 050.00 · 1.000 = 5 005 000 центов · 7 bps = 3503.5 → 3504.
  assert.equal(Tariff.commissionMinor(5_005_000, 1000, 2, 3, 7), 3504);
  assert.equal(Tariff.commissionMinor(PX, 1000, 2, 3, 0), 0);
  // BTC: 0.01 по 50 000.00 = 500 USD · 10 bps = 0.50 USD.
  assert.equal(Tariff.commissionMinor(PX, 1000, 2, 5, 10), 50);
});

test('строка тикета: есть комиссия — сумма; нет тарифа — строки нет', () => {
  assert.equal(Tariff.commissionText(50, 10), 'Commission ≈ 0.50 USD');
  assert.equal(Tariff.commissionText(0, 0), '');
  assert.equal(Tariff.commissionText(NaN, 10), 'Commission ≈ — USD', 'цены нет — прочерк, не ноль');
});
