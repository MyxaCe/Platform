'use strict';
// ============================================================================
// Ряд цены и его разрыв (TRM-19, Р-073).
//
// Проверяется СВОЙСТВО, а не сегодняшние числа: после разрыва ряда сделка не
// может попасть в свечу, которая формировалась до разрыва. Это и есть всё
// решение штаба, выраженное одним утверждением.
//
// Почему это стоит отдельного набора: до выноса в модуль агрегатор жил
// строкой в `app.js`, был непроверяем вовсе, и именно в нём бар переживал
// обрыв сокета — `ws.onclose` не сбрасывал ничего.
// ============================================================================
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadModule } = require('./helpers/load-module');

const Series = loadModule('series.js', 'Series');

/** Бар, как его отдаёт `/candles`, в raw-единицах. */
function bar(over) {
  return Object.assign({ time: 1000, open: 100, high: 100, low: 100, close: 100, volume: 5 }, over);
}

const TF = 60;

test('в пределах интервала свеча растёт: максимум, минимум и цена закрытия', () => {
  const s = Series.create();
  s.seed(bar({ time: 1020 }));
  s.onTrade({ now: 1030, tf: TF, price: 130, qty: 1 });
  s.onTrade({ now: 1040, tf: TF, price: 70, qty: 1 });
  const f = s.onTrade({ now: 1050, tf: TF, price: 110, qty: 1 });

  assert.equal(f.time, 1020, 'тот же интервал — та же свеча');
  assert.equal(f.high, 130, 'максимум держится по всем сделкам интервала');
  assert.equal(f.low, 70, 'минимум — тоже');
  assert.equal(f.close, 110, 'закрытие — последняя сделка');
});

test('новый интервал открывает новую свечу', () => {
  const s = Series.create();
  s.seed(bar({ time: 1020 }));
  const f = s.onTrade({ now: 1081, tf: TF, price: 130, qty: 2 });

  assert.equal(f.time, 1080, 'время свечи — граница интервала');
  assert.equal(f.open, 130, 'открытие новой свечи — цена первой сделки');
  assert.equal(f.high, 130);
  assert.equal(f.low, 130);
});

// --- собственно разрыв -----------------------------------------------------

test('ПОСЛЕ РАЗРЫВА сделка не дописывается в свечу, которая формировалась до него', () => {
  const s = Series.create();
  s.seed(bar({ time: 1020, high: 100, low: 100, close: 100 }));
  s.onTrade({ now: 1030, tf: TF, price: 105, qty: 1 });

  s.markBreak('feed-lost');

  // Сделка того же интервала — то есть ровно тот случай, когда старый код
  // продолжал прежний бар через дыру.
  const after = s.onTrade({ now: 1050, tf: TF, price: 900, qty: 1 });

  assert.equal(after, null, 'рисовать нечего: ряд разорван');
  assert.equal(s.forming().high, 105, 'движение из-за дыры в наблюдённую свечу не попало');
  assert.equal(s.forming().close, 105, 'закрытие осталось последним НАБЛЮДЁННЫМ');
});

test('разрыв закрывает свечу, но не стирает наблюдённое', () => {
  const s = Series.create();
  s.seed(bar({ time: 1020 }));
  s.onTrade({ now: 1030, tf: TF, price: 115, qty: 1 });

  const r = s.markBreak('feed-stale');

  assert.equal(r.reason, 'feed-stale');
  assert.equal(r.closed.high, 115, 'закрытая свеча отдана такой, какой её видели');
  assert.equal(s.forming().high, 115, 'и на графике она остаётся');
  assert.equal(s.isBroken(), true);
  assert.equal(s.breakReason(), 'feed-stale');
});

test('пересев с сервера снимает разрыв и возвращает ряд в работу', () => {
  const s = Series.create();
  s.seed(bar({ time: 1020 }));
  s.markBreak('feed-lost');
  assert.equal(s.onTrade({ now: 1050, tf: TF, price: 900, qty: 1 }), null, 'до пересева — нет');

  // REST видел дыру целиком, поэтому его ответ авторитетен.
  s.seed(bar({ time: 1080, open: 300, high: 300, low: 300, close: 300 }));

  assert.equal(s.isBroken(), false, 'разрыв снят');
  assert.equal(s.breakReason(), null);
  const f = s.onTrade({ now: 1090, tf: TF, price: 310, qty: 1 });
  assert.equal(f.high, 310, 'агрегация продолжилась от пересеянной свечи');
});

test('пустой ответ сервера пересевает ряд в пустоту, а не оставляет старую свечу', () => {
  const s = Series.create();
  s.seed(bar({ time: 1020 }));
  s.seed(null);

  assert.equal(s.forming(), null);
  assert.equal(s.lastBarTime(), 0);
});

test('второй разрыв подряд не стирает уже закрытую свечу', () => {
  // Обрыв и протухание приходят парой: сокет закрылся, следом сработал порог
  // молчания. Вторая причина не должна отменять наблюдённое до первой.
  const s = Series.create();
  s.seed(bar({ time: 1020 }));
  s.onTrade({ now: 1030, tf: TF, price: 115, qty: 1 });

  s.markBreak('feed-lost');
  const second = s.markBreak('feed-stale');

  assert.equal(second.closed.high, 115, 'свеча на месте');
  assert.equal(s.breakReason(), 'feed-lost', 'причина — ПЕРВАЯ: она объясняет разрыв');
});

// --- словарь причин --------------------------------------------------------

test('причина не из словаря — отказ, а не молчаливый разрыв по синониму', () => {
  const s = Series.create();
  assert.throws(() => s.markBreak('unreachable'), /неизвестная причина разрыва ряда/);
  assert.throws(() => s.markBreak('source_change'), /неизвестная причина разрыва ряда/, 'snake_case — чужой стиль');
});

test('причина `source-change` заведена заранее и в день переезда на MDS не упрётся в словарь', () => {
  // Сегодня её некому породить: терминал берёт цены прямо с биржи и поля
  // `source` не видит. Место названо, чтобы его не искали заново.
  const s = Series.create();
  s.seed(bar({ time: 1020 }));
  assert.doesNotThrow(() => s.markBreak('source-change'));
  assert.equal(s.breakReason(), 'source-change');
});

// --- замкнутость состояния -------------------------------------------------

test('свеча отдаётся копией: снаружи её не дописать в обход разрыва', () => {
  // Та же грабля, что породила исходную ошибку: состояние ряда лежало
  // модульной переменной, и править его мог кто угодно откуда угодно.
  const s = Series.create();
  s.seed(bar({ time: 1020, high: 100 }));

  const stolen = s.forming();
  stolen.high = 9000;

  assert.equal(s.forming().high, 100, 'внутреннее состояние не тронуто');
});

test('сделка из прошлого интервала не сдвигает границы текущей свечи задним числом', () => {
  const s = Series.create();
  s.seed(bar({ time: 1080, high: 100, low: 100 }));
  const late = s.onTrade({ now: 1030, tf: TF, price: 900, qty: 1 });

  assert.equal(late, null, 'опоздавшая сделка отброшена');
  assert.equal(s.forming().high, 100, 'текущая свеча не переписана прошлым');
});
