'use strict';
// ============================================================================
// Общий помощник диагностики (TRM-17).
//
// Главное свойство здесь не «пишет в лог», а **одна политика на всех**. Политик
// было изобретено три разными заходами, каждая защитима для своей частоты — и
// именно поэтому читатель консоли не знал, какую видит.
//
// Второе свойство: словарь причин один и он **краснеет на неизвестном слове**.
// Молчаливый пропуск и есть способ, которым два словаря расходятся годами.
// ============================================================================
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadModule } = require('./helpers/load-module');

const Diag = loadModule('diag.js', 'Diag');

/** Журнал вместо консоли: проверяем, что именно сказано и сколько раз. */
function recorder() {
  const errors = [];
  const infos = [];
  return { log: { error: (...a) => errors.push(a), info: (...a) => infos.push(a) }, errors, infos };
}

// ---- Словарь ---------------------------------------------------------------

test('словарь — ровно ось канала из КОНТРАКТЫ §4г', () => {
  // Ось символа (`no-source`, `stale`, `source-disabled`) сюда НЕ входит: это
  // другой вопрос, и называет его MDS. Сведи их в один словарь — и «нет связи»
  // окажется тем же словом, что «источник выключен».
  assert.deepEqual(
    Object.keys(Diag.CHANNEL).sort(),
    ['empty', 'http-error', 'malformed', 'network', 'not-configured'],
  );
});

test('неизвестная причина — ошибка, а не молчаливый пропуск', () => {
  // Фильтр, глотающий неизвестное слово, и есть механизм, которым расхождение
  // живёт годами: один пишет `unreachable`, другой `network`, оба проходят.
  assert.throws(() => Diag.describe('unreachable'), /не из словаря оси канала/);
  assert.throws(() => Diag.describe('source-disabled'), /не из словаря/);
  assert.throws(() => Diag.describe(''), /не из словаря/);
});

test('сообщение об ошибке называет, куда добавлять новое имя', () => {
  // Иначе следующий добавит слово в код, и словарей снова станет два.
  assert.throws(() => Diag.describe('нечто'), /КОНТРАКТЫ\.md §4г/);
});

test('сеть и отказ соседа — разные слова и разные подписи', () => {
  // Первое чинит наша сторона, второе — сосед. Схлопнутые, они отправляют
  // чинить наугад.
  const net = Diag.describe('network');
  const http = Diag.describe('http-error');

  assert.notEqual(net.badge, http.badge);
  assert.notEqual(net.why, http.why);
});

// ---- Политика по умолчанию -------------------------------------------------

test('по умолчанию пишем на смене состояния, а не на каждом тике', () => {
  const r = recorder();
  const d = Diag.reporter({ subject: 'проба', log: r.log });

  for (let i = 0; i < 5; i += 1) d.fail('network', null);

  assert.equal(r.errors.length, 1, `одна строка на серию, а не ${r.errors.length}`);
  assert.equal(d.count(), 5, 'повторы при этом посчитаны');
});

test('смена причины — новая строка: это другая неполадка', () => {
  const r = recorder();
  const d = Diag.reporter({ subject: 'проба', log: r.log });

  d.fail('network', null);
  d.fail('network', null);
  d.fail('http-error', null);

  assert.equal(r.errors.length, 2, 'переход network → http-error обязан быть слышен');
});

test('восстановление называется числом неудач', () => {
  // «Снова работает» без числа не даёт понять, моргнуло один раз или лежало час.
  const r = recorder();
  const d = Diag.reporter({ subject: 'проба', log: r.log });

  d.fail('network', null);
  d.fail('network', null);
  const was = d.ok();

  assert.equal(was, 'network');
  assert.equal(r.infos.length, 1);
  assert.match(String(r.infos[0][0]), /после 2 неудач/);
});

test('успех без предшествующих отказов молчит', () => {
  const r = recorder();
  const d = Diag.reporter({ subject: 'проба', log: r.log });

  d.ok();
  d.ok();

  assert.deepEqual(r.infos, [], 'нечего сообщать — нечего и писать');
});

test('состояние сбрасывается после восстановления', () => {
  const r = recorder();
  const d = Diag.reporter({ subject: 'проба', log: r.log });

  d.fail('network', null);
  d.ok();
  assert.equal(d.state(), null);
  assert.equal(d.count(), 0);

  d.fail('network', null);
  assert.equal(r.errors.length, 2, 'после восстановления та же причина — снова новость');
});

// ---- Отличающаяся политика задаётся, а не пишется заново --------------------

test('частая политика задаётся параметром', () => {
  // Разбор сообщений фида идёт на порядок чаще опроса, и там «первое и каждое
  // N-е» уместно. Уместно — значит задаётся, а не пишется рядом заново.
  const r = recorder();
  const d = Diag.reporter({ subject: 'фид', every: 3, log: r.log });

  for (let i = 0; i < 7; i += 1) d.fail('malformed', null);

  // 1-й (смена состояния), 3-й и 6-й (каждый третий).
  assert.equal(r.errors.length, 3, `ожидали 3 строки, получили ${r.errors.length}`);
});

test('хост получает каждый отказ, даже когда лог молчит', () => {
  // Политика относится к ЛОГУ. У хоста свой индикатор, и пропускать его тики
  // значило бы решать за него, что ему важно.
  const r = recorder();
  const seen = [];
  const d = Diag.reporter({ subject: 'проба', log: r.log, onError: (e) => seen.push(e) });

  d.fail('network', null);
  d.fail('network', null);
  d.fail('network', null);

  assert.equal(r.errors.length, 1, 'лог молчит после первой');
  assert.equal(seen.length, 3, 'а хост знает про все три');
  assert.deepEqual(seen.map((e) => e.reason), ['network', 'network', 'network']);
  assert.deepEqual(seen.map((e) => e.first), [true, false, false]);
});

test('fail возвращает подпись, и виджету остаётся только нарисовать', () => {
  // Если бы подпись каждый виджет сочинял сам, в стакане и в полосе
  // статистики одна и та же неполадка называлась бы по-разному.
  const r = recorder();
  const d = Diag.reporter({ subject: 'проба', log: r.log });

  const out = d.fail('http-error', 503);

  assert.equal(out.badge, Diag.CHANNEL['http-error'].badge);
  assert.equal(out.why, Diag.CHANNEL['http-error'].why);
  assert.equal(out.first, true);
});
