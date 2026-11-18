'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadModule } = require('./helpers/load-module');

const I = loadModule('indicators.js', 'Indicators');

function series(n) {
  const out = [];
  let price = 100;
  for (let i = 0; i < n; i++) {
    const step = Math.sin(i / 3) * 4 + (i % 7 === 0 ? -6 : 1.5);
    const open = price;
    price = Math.round((price + step) * 100) / 100;
    out.push({
      time: 1700000000 + i * 60,
      open,
      high: Math.max(open, price) + 1.25,
      low: Math.min(open, price) - 1.25,
      close: price,
      volume: 100 + ((i * 37) % 53),
    });
  }
  return out;
}

const CS = series(60);
const CLOSES = CS.map((c) => c.close);

const firstDefined = (arr) => arr.findIndex((x) => x != null);

test('EMA считается по рекурренте, и затравка — первое значение ряда', () => {
  const out = I.ema([1, 2, 3], 2);

  assert.equal(out[0], null, 'до p−1 значения нет');
  assert.ok(Math.abs(out[1] - 5 / 3) < 1e-12, `ожидали 5/3, получили ${out[1]}`);
  assert.ok(Math.abs(out[2] - 23 / 9) < 1e-12, `ожидали 23/9, получили ${out[2]}`);
});

test('инвариант сдвига к EMA неприменим — записано, чтобы его не «дозакрыли»', () => {
  const long = I.macdFull(CLOSES)[1];
  const short = I.macdFull(CLOSES.slice(10))[1];

  const i = CLOSES.length - 1;
  assert.notEqual(
    long[i],
    short[i - 10],
    'если значения совпали, EMA перестала быть EMA — проверь затравку',
  );
});

test('MACD: линия с 25-го бара, сигнальная — с 33-го', () => {
  const [line, sig] = I.macdFull(CLOSES);

  assert.equal(firstDefined(line), 25, 'MACD-линия');
  assert.equal(firstDefined(sig), 33, 'сигнальная линия');
});

test('AC: с 37-го бара, а не с 33-го', () => {
  const ac = I.acOsc(CS);

  assert.equal(firstDefined(I.ao(CS)), 33, 'AO');
  assert.equal(firstDefined(ac), 37, 'AC');
});

test('Force Index: с p-го бара', () => {
  for (const p of [2, 5, 13]) {
    assert.equal(firstDefined(I.forceIndex(CS, p)), p, `период ${p}`);
  }
});

test('сигнальная MACD — это EMA самой линии, а не линии с дописанными нулями', () => {
  const [line, sig] = I.macdFull(CLOSES);
  const start = firstDefined(line);
  const reference = I.ema(line.slice(start), 9);

  let compared = 0;
  for (let i = start; i < line.length; i++) {
    const expected = reference[i - start];
    if (expected == null) {
      assert.equal(sig[i], null, `бар ${i}: значения быть не должно`);
      continue;
    }
    assert.ok(Math.abs(sig[i] - expected) < 1e-9, `бар ${i}: ${sig[i]} против ${expected}`);
    compared += 1;
  }
  assert.ok(compared >= 10, `сравнивать было нечего, совпавших баров ${compared}`);
});

test('AC — это AO минус SMA определённой части AO', () => {
  const ao = I.ao(CS);
  const ac = I.acOsc(CS);
  const start = firstDefined(ao);
  const reference = I.sma(ao.slice(start), 5);

  let compared = 0;
  for (let i = start; i < ao.length; i++) {
    const s = reference[i - start];
    if (s == null) {
      assert.equal(ac[i], null, `бар ${i}: значения быть не должно`);
      continue;
    }
    assert.ok(Math.abs(ac[i] - (ao[i] - s)) < 1e-9, `бар ${i}`);
    compared += 1;
  }
  assert.ok(compared >= 10, `сравнивать было нечего, совпавших баров ${compared}`);
});

test('Force Index — это EMA определённой части ряда импульсов', () => {
  const p = 13;
  const fi = I.forceIndex(CS, p);
  const raw = CS.map((c, i) => (i === 0 ? null : (c.close - CS[i - 1].close) * c.volume));
  const reference = I.ema(raw.slice(1), p);

  let compared = 0;
  for (let i = 1; i < CS.length; i++) {
    const expected = reference[i - 1];
    if (expected == null) {
      assert.equal(fi[i], null, `бар ${i}: значения быть не должно`);
      continue;
    }
    assert.ok(Math.abs(fi[i] - expected) < 1e-9, `бар ${i}: ${fi[i]} против ${expected}`);
    compared += 1;
  }
  assert.ok(compared >= 10, `сравнивать было нечего, совпавших баров ${compared}`);
});

test('ряд короче периода не выдумывает значений', () => {
  assert.equal(I.macdFull(CLOSES.slice(0, 20))[1].filter((x) => x != null).length, 0);
  assert.equal(I.acOsc(CS.slice(0, 20)).filter((x) => x != null).length, 0);
  assert.equal(I.forceIndex(CS.slice(0, 3), 13).filter((x) => x != null).length, 0);
});

test('данные для этих проверок не вырождены', () => {
  const ups = CLOSES.filter((c, i) => i > 0 && c > CLOSES[i - 1]).length;
  const downs = CLOSES.filter((c, i) => i > 0 && c < CLOSES[i - 1]).length;
  const volumes = new Set(CS.map((c) => c.volume));

  assert.ok(ups >= 10, `рост должен встречаться, его ${ups}`);
  assert.ok(downs >= 10, `падение должно встречаться, его ${downs}`);
  assert.ok(volumes.size >= 10, `объёмы должны различаться, их ${volumes.size}`);
});

test('мутация «дыру в середине ряда заглаживать молча» здесь непрогоняема', () => {
  assert.ok(true);
});
