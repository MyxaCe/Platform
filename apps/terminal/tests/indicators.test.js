'use strict';
// ============================================================================
// Тесты индикаторов (TRM-05).
//
// Почему это вообще нужно: ошибка в индикаторе не падает и не пишет в консоль —
// она рисует НЕПРАВИЛЬНУЮ ЛИНИЮ, по которой человек принимает решение. Такие
// дефекты живут годами; три давних дефекта UI (BUG-008…010) нашлись только при
// рефакторинге, а существовали задолго до него.
//
// Эталонные значения посчитаны НЕЗАВИСИМО от кода терминала — по учебным
// определениям, отдельным скриптом. Если брать ожидания из самой реализации,
// тест закрепит её ошибку и будет вечно зелёным.
//
// Где формула допускает варианты (затравка EMA, знаменатель СКО), проверяется не
// только значение, но и ОПРЕДЕЛЯЮЩЕЕ СВОЙСТВО (рекуррента, инвариант) — свойство
// ломается при любой ошибке, а не только при отклонении от нашей серии.
//
// Запуск: node --test apps/terminal/tests/
// ============================================================================
const test = require('node:test');
const assert = require('node:assert/strict');

const { loadModule } = require('./helpers/load-module');
const { CANDLES, CLOSES } = require('./fixtures/candles');

const I = loadModule('indicators.js', 'Indicators');

/** Сравнение массивов с допуском; null обязан совпасть с null. */
function eqSeries(actual, expected, label, eps = 1e-6) {
  assert.equal(actual.length, expected.length, `${label}: длина ряда`);
  for (let i = 0; i < expected.length; i++) {
    const e = expected[i];
    const a = actual[i];
    if (e === null) {
      assert.equal(a, null, `${label}[${i}]: ожидался null (значение не определено)`);
    } else {
      assert.notEqual(a, null, `${label}[${i}]: значение пропущено, ожидалось ${e}`);
      assert.ok(Math.abs(a - e) < eps, `${label}[${i}]: ожидали ${e}, получили ${a}`);
    }
  }
}

// ============================ Скользящие средние ============================

test('sma(3) — среднее последних трёх, до полного окна null', () => {
  eqSeries(I.sma(CLOSES, 3), [
    null, null, 11.166667, 11.0, 10.166667, 10.166667,
    11.133333, 12.1, 13.133333, 13.333333, 14.466667, 14.7,
  ], 'sma3');
});

test('sma — окно не «съезжает»: на константе равна константе', () => {
  const flat = new Array(8).fill(7);
  eqSeries(I.sma(flat, 4), [null, null, null, 7, 7, 7, 7, 7], 'sma-const');
});

test('wma(3) — веса 3,2,1 (последняя цена весит больше всех)', () => {
  eqSeries(I.wma(CLOSES, 3), [
    null, null, 11.283333, 10.7, 9.85, 10.366667,
    11.733333, 12.266667, 13.366667, 13.55, 14.683333, 14.95,
  ], 'wma3');
});

test('wma тяжелее реагирует на последнюю цену, чем sma', () => {
  // Инвариант, а не значение: при росте последней цены WMA обязана быть выше SMA.
  const rising = [1, 2, 3, 4, 5, 6];
  const s = I.sma(rising, 3);
  const w = I.wma(rising, 3);
  for (let i = 2; i < rising.length; i++) {
    assert.ok(w[i] > s[i], `wma[${i}] должна опережать sma на растущем ряде`);
  }
});

test('ema(3) — рекуррента с k = 2/(p+1)', () => {
  const out = I.ema(CLOSES, 3);
  eqSeries(out, [
    null, null, 11.175, 10.5875, 9.94375, 10.571875,
    11.735938, 11.967969, 13.133984, 13.316992, 14.458496, 14.729248,
  ], 'ema3');

  // Определяющее свойство: каждое следующее значение — взвешенная сумма новой
  // цены и предыдущего значения. Оно ломается при любой ошибке в коэффициенте,
  // даже если затравку поменяют (у нас затравка — первое значение ряда).
  const k = 2 / (3 + 1);
  for (let i = 3; i < out.length; i++) {
    const expected = CLOSES[i] * k + out[i - 1] * (1 - k);
    assert.ok(Math.abs(out[i] - expected) < 1e-9, `ema[${i}]: нарушена рекуррента`);
  }
});

test('smma(3) — сглаживание Уайлдера, затравка средним первых p', () => {
  const out = I.smma(CLOSES, 3);
  assert.equal(out[0], null);
  assert.equal(out[1], null);
  assert.ok(Math.abs(out[2] - (CLOSES[0] + CLOSES[1] + CLOSES[2]) / 3) < 1e-9, 'затравка smma');
  for (let i = 3; i < out.length; i++) {
    const expected = (out[i - 1] * 2 + CLOSES[i]) / 3;
    assert.ok(Math.abs(out[i] - expected) < 1e-9, `smma[${i}]: нарушена рекуррента Уайлдера`);
  }
});

// ============================ Волатильность =================================

test('stdev(3) — популяционное СКО (делитель p, не p−1)', () => {
  eqSeries(I.stdev(CLOSES, 3), [
    null, null, 0.531246, 0.748331, 0.784573, 0.784573,
    1.47045, 0.697615, 0.873053, 0.865384, 0.865384, 0.883176,
  ], 'sd3');
});

test('stdev на константе равен нулю', () => {
  const out = I.stdev(new Array(6).fill(5), 3);
  for (let i = 2; i < out.length; i++) assert.ok(Math.abs(out[i]) < 1e-12, 'СКО константы = 0');
});

test('trueRange — берёт максимум из трёх ветвей, включая гэпы', () => {
  eqSeries(I.trueRange(CANDLES), [1.5, 1.8, 1.4, 1.7, 1.6, 2.3, 1.9, 1.2, 2.4, 1.4, 2.4, 1.2], 'tr');
});

test('trueRange учитывает разрыв к предыдущему close, а не только high−low', () => {
  // Свеча целиком выше предыдущего close: TR обязан мерить от close, иначе
  // волатильность гэпа теряется и ATR занижается.
  const gap = [
    { high: 10, low: 9, close: 9.5 },
    { high: 20, low: 19, close: 19.5 },
  ];
  const tr = I.trueRange(gap);
  assert.equal(tr[1], 20 - 9.5, 'TR через гэп = high − предыдущий close');
});

test('atr(3) — сглаживание Уайлдера по True Range', () => {
  eqSeries(I.atr(CANDLES, 3), [
    null, null, 1.566667, 1.611111, 1.607407, 1.838272,
    1.858848, 1.639232, 1.892821, 1.728547, 1.952365, 1.701577,
  ], 'atr3');
});

// ============================ Осцилляторы ===================================

test('rsi(3) — сглаживание Уайлдера, первое значение на баре p', () => {
  eqSeries(I.rsi(CLOSES, 3), [
    null, null, null, 41.935484, 31.325301, 66.172107,
    79.876434, 63.889869, 81.001021, 63.741323, 80.283509, 67.153303,
  ], 'rsi3');
});

test('rsi на монотонном росте стремится к 100, на падении — к 0', () => {
  const up = I.rsi([1, 2, 3, 4, 5, 6, 7, 8], 3);
  const down = I.rsi([8, 7, 6, 5, 4, 3, 2, 1], 3);
  assert.ok(up[up.length - 1] > 99, 'только рост → RSI ≈ 100');
  assert.ok(down[down.length - 1] < 1, 'только падение → RSI ≈ 0');
});

test('rsi всегда в диапазоне 0..100', () => {
  for (const v of I.rsi(CLOSES, 3)) {
    if (v != null) assert.ok(v >= 0 && v <= 100, `RSI вне диапазона: ${v}`);
  }
});

test('stoch(3,2) — %K по окну high/low, %D — среднее %K', () => {
  const [k, d] = I.stoch(CANDLES, 3, 2);
  eqSeries(k, [
    null, null, 58.62069, 7.692308, 8.823529, 88.0,
    97.5, 75.609756, 94.117647, 57.692308, 94.594595, 66.666667,
  ], 'stochK');
  // %D — скользящее среднее %K, проверяем как свойство.
  for (let i = 3; i < d.length; i++) {
    if (d[i] == null) continue;
    assert.ok(Math.abs(d[i] - (k[i] + k[i - 1]) / 2) < 1e-9, `%D[${i}] должен быть средним %K`);
  }
});

test('williams(3) = %K − 100 (тот же расчёт, другой отсчёт)', () => {
  const w = I.williams(CANDLES, 3);
  const [k] = I.stoch(CANDLES, 3, 2);
  for (let i = 0; i < w.length; i++) {
    if (w[i] == null || k[i] == null) continue;
    assert.ok(Math.abs(w[i] - (k[i] - 100)) < 1e-9, `williams[${i}] должен равняться %K−100`);
  }
  eqSeries(w, [
    null, null, -41.37931, -92.307692, -91.176471, -12.0,
    -2.5, -24.390244, -5.882353, -42.307692, -5.405405, -33.333333,
  ], 'willR');
});

test('momentum(3) и roc(3) — разность и та же разность в процентах', () => {
  eqSeries(I.momentum(CLOSES, 3), [null, null, null, -0.5, -2.5, 0, 2.9, 2.9, 3.1, 0.6, 3.4, 0.7], 'mom3');
  eqSeries(I.roc(CLOSES, 3), [
    null, null, null, -4.761905, -21.186441, 0,
    29.0, 31.182796, 27.678571, 4.651163, 27.868852, 4.895105,
  ], 'roc3');
});

test('macdFull — линия = ema(12) − ema(26), сигнал = ema(9) от линии', () => {
  const long = CLOSES.concat(CLOSES, CLOSES); // 36 точек: хватает на ema(26)
  const [line, signal] = I.macdFull(long);
  const f = I.ema(long, 12);
  const s = I.ema(long, 26);
  for (let i = 25; i < long.length; i++) {
    assert.ok(Math.abs(line[i] - (f[i] - s[i])) < 1e-9, `macd[${i}] = ema12 − ema26`);
  }
  assert.ok(signal.some((v) => v != null), 'сигнальная линия обязана считаться');
});

// ============================ Объём =========================================

test('obv — накопление объёма по знаку изменения цены', () => {
  eqSeries(I.obv(CANDLES), [0, 140, 50, -150, -330, -70, 230, 110, 450, 290, 670, 560], 'obv');
});

test('obv не меняется, когда цена не изменилась', () => {
  const flat = [
    { close: 10, volume: 100 },
    { close: 10, volume: 999 },
    { close: 10, volume: 999 },
  ];
  const out = I.obv(flat);
  assert.equal(out[2], out[0], 'без движения цены OBV стоит');
});

test('adLine — накопленный денежный поток по положению close в диапазоне', () => {
  eqSeries(I.adLine(CANDLES), [
    33.333333, 142.222222, 77.936508, -75.004669, -187.504669, 27.27794,
    295.698993, 215.698993, 499.032326, 407.603755, 724.270421, 669.270421,
  ], 'adLine');
});

test('cmf(3) — тот же поток, нормированный объёмом окна; всегда в −1..1', () => {
  const out = I.cmf(CANDLES, 3);
  eqSeries(out, [
    null, null, 0.236171, -0.251949, -0.701547, -0.079154,
    0.500951, 0.592947, 0.620729, 0.180492, 0.577922, 0.261905,
  ], 'cmf3');
  for (const v of out) {
    if (v != null) assert.ok(v >= -1 && v <= 1, `CMF вне диапазона: ${v}`);
  }
});

test('mfi(3) — RSI по денежному потоку, диапазон 0..100', () => {
  eqSeries(I.mfi(CANDLES, 3), [
    null, null, null, 55.709636, 21.363886, 41.82907,
    78.805411, 100, 100, 100, 100, 100,
  ], 'mfi3');
});

test('vwap — кумулятивная средняя по HLC3, взвешенная объёмом', () => {
  eqSeries(I.vwap(CANDLES)[0].values, [
    10.333333, 10.916667, 11.084848, 10.838994, 10.533333, 10.542268,
    10.965354, 11.094964, 11.593834, 11.777778, 12.306021, 12.439776,
  ], 'vwap');
});

// ============================ Каналы и наложения ============================

// Боллинджер, Кельтнер и Дончиан зашиты на период 20 (параметра у них нет),
// поэтому им нужна серия длиннее 20 баров — фикстура из 12 даёт сплошные null.
const LONG = CANDLES.concat(CANDLES, CANDLES);
const LONG_CLOSES = LONG.map((c) => c.close);

test('bollinger — середина есть sma(20), полосы симметричны и отстоят на 2 СКО', () => {
  const bands = I.bollinger(LONG);
  assert.equal(bands.length, 3, 'полосы Боллинджера: верх, середина, низ');
  const [up, ml, lo] = bands.map((b) => b.values);
  const sma20 = I.sma(LONG_CLOSES, 20);
  const sd20 = I.stdev(LONG_CLOSES, 20);
  let checked = 0;
  for (let i = 0; i < LONG.length; i++) {
    if (ml[i] == null) continue;
    checked++;
    assert.ok(Math.abs(ml[i] - sma20[i]) < 1e-9, `середина[${i}] должна быть sma(20)`);
    assert.ok(Math.abs(up[i] - (ml[i] + 2 * sd20[i])) < 1e-9, `верх[${i}] = середина + 2σ`);
    assert.ok(Math.abs((up[i] - ml[i]) - (ml[i] - lo[i])) < 1e-9, `полосы[${i}] несимметричны`);
  }
  assert.ok(checked > 0, 'ни одного посчитанного бара — тест ничего не проверил');
});

test('keltner — середина есть ema(20), канал строится по ATR', () => {
  const [up, ml, lo] = I.keltner(LONG).map((b) => b.values);
  const ema20 = I.ema(LONG_CLOSES, 20);
  const atr20 = I.atr(LONG, 20);
  let checked = 0;
  for (let i = 0; i < LONG.length; i++) {
    if (ml[i] == null || atr20[i] == null) continue;
    checked++;
    assert.ok(Math.abs(ml[i] - ema20[i]) < 1e-9, `середина[${i}] должна быть ema(20)`);
    assert.ok(Math.abs(up[i] - (ml[i] + 2 * atr20[i])) < 1e-9, `верх[${i}] = середина + 2·ATR`);
    assert.ok(Math.abs(lo[i] - (ml[i] - 2 * atr20[i])) < 1e-9, `низ[${i}] = середина − 2·ATR`);
  }
  assert.ok(checked > 0, 'ни одного посчитанного бара');
});

test('donchian — верх есть максимум high, низ есть минимум low окна 20', () => {
  const lines = I.donchian(LONG);
  const [up, mid, lo] = lines.map((b) => b.values);
  let checked = 0;
  for (let i = 0; i < LONG.length; i++) {
    if (up[i] == null) { assert.ok(i < 19, `бар ${i}: значение пропущено без причины`); continue; }
    checked++;
    assert.equal(up[i], I.highest(LONG, 20, i), `верх[${i}] — максимум high окна`);
    assert.equal(lo[i], I.lowest(LONG, 20, i), `низ[${i}] — минимум low окна`);
    assert.ok(Math.abs(mid[i] - (up[i] + lo[i]) / 2) < 1e-9, `середина[${i}] — центр канала`);
    assert.ok(up[i] >= lo[i], `канал[${i}] вывернут наизнанку`);
  }
  assert.ok(checked > 0, 'ни одного посчитанного бара');
});

test('highest/lowest — границы окна, включая текущий бар', () => {
  assert.equal(I.highest(CANDLES, 3, 2), 12.4);
  assert.equal(I.lowest(CANDLES, 3, 2), 9.5);
  assert.equal(I.highest(CANDLES, 1, 5), CANDLES[5].high, 'окно 1 = сам бар');
});

test('HL2 и HLC3 — средние цены бара', () => {
  assert.equal(I.HL2(CANDLES[0]), (11.0 + 9.5) / 2);
  assert.ok(Math.abs(I.HLC3(CANDLES[0]) - (11.0 + 9.5 + 10.5) / 3) < 1e-9);
});

test('heikin — сглаженные свечи: close = среднее OHLC, open = среднее предыдущей', () => {
  const ha = I.heikin(CANDLES);
  eqSeries(ha.map((c) => c.close), [
    10.25, 11.125, 11.6, 10.625, 9.725, 10.25, 12.05, 12.575, 13.275, 13.9, 14.575, 15.3,
  ], 'haClose');
  eqSeries(ha.map((c) => c.open), [
    10.25, 10.25, 10.6875, 11.14375, 10.884375, 10.304688,
    10.277344, 11.163672, 11.869336, 12.572168, 13.236084, 13.905542,
  ], 'haOpen');
  // Свойство: high свечи Heikin не ниже её тела.
  for (const c of ha) {
    assert.ok(c.high >= Math.max(c.open, c.close) - 1e-9, 'high ниже тела');
    assert.ok(c.low <= Math.min(c.open, c.close) + 1e-9, 'low выше тела');
  }
});

test('ichimoku — tenkan и kijun суть середины своих окон', () => {
  const long = CANDLES.concat(CANDLES, CANDLES, CANDLES, CANDLES, CANDLES); // 72 бара
  const [tenkan, kijun] = I.ichimoku(long);
  for (let i = 8; i < long.length; i++) {
    const mid9 = (I.highest(long, 9, i) + I.lowest(long, 9, i)) / 2;
    assert.ok(Math.abs(tenkan.values[i] - mid9) < 1e-9, `tenkan[${i}] — середина окна 9`);
  }
  for (let i = 25; i < long.length; i++) {
    const mid26 = (I.highest(long, 26, i) + I.lowest(long, 26, i)) / 2;
    assert.ok(Math.abs(kijun.values[i] - mid26) < 1e-9, `kijun[${i}] — середина окна 26`);
  }
});

test('alligator — три smma по HL2 с разными периодами', () => {
  const lines = I.alligator(CANDLES);
  assert.equal(lines.length, 3, 'челюсти, зубы, губы');
  const mids = CANDLES.map(I.HL2);
  const expected = [13, 8, 5].map((p) => I.smma(mids, p));
  lines.forEach((line, idx) => {
    eqSeries(line.values, expected[idx], `alligator[${idx}]`);
  });
});

// ============================ Общие свойства ================================

test('индикаторы не портят входные данные', () => {
  const before = JSON.stringify(CANDLES);
  I.atr(CANDLES, 3); I.obv(CANDLES); I.heikin(CANDLES); I.stoch(CANDLES, 3, 2); I.vwap(CANDLES);
  assert.equal(JSON.stringify(CANDLES), before, 'свечи изменены индикатором');
});

test('короткий ряд не роняет индикаторы и не выдумывает значения', () => {
  const two = CANDLES.slice(0, 2);
  const twoCloses = CLOSES.slice(0, 2);
  // Сравниваем через eqSeries, а не deepStrictEqual: массивы приходят из
  // vm-песочницы, у них СВОЙ Array из другого realm, и строгое сравнение падает
  // на прототипе при полностью совпадающем содержимом.
  eqSeries(I.sma(twoCloses, 5), [null, null], 'sma-короткий');
  eqSeries(I.stdev(twoCloses, 5), [null, null], 'stdev-короткий');
  eqSeries(I.atr(two, 5), [null, null], 'atr-короткий');
  eqSeries(I.rsi(twoCloses, 5), [null, null], 'rsi-короткий');
  assert.equal(I.obv(two).length, 2);
});
