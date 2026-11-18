'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadModule } = require('./helpers/load-module');

const Diag = loadModule('diag.js', 'Diag');

function recorder() {
  const errors = [];
  const infos = [];
  return { log: { error: (...a) => errors.push(a), info: (...a) => infos.push(a) }, errors, infos };
}

test('словарь — ровно ось канала из КОНТРАКТЫ §4г', () => {
  assert.deepEqual(
    Object.keys(Diag.CHANNEL).sort(),
    ['empty', 'http-error', 'malformed', 'network', 'not-configured'],
  );
});

test('неизвестная причина — ошибка, а не молчаливый пропуск', () => {
  assert.throws(() => Diag.describe('unreachable'), /не из словаря оси канала/);
  assert.throws(() => Diag.describe('source-disabled'), /не из словаря/);
  assert.throws(() => Diag.describe(''), /не из словаря/);
});

test('сообщение об ошибке называет, куда добавлять новое имя', () => {
  assert.throws(() => Diag.describe('нечто'), /КОНТРАКТЫ\.md §4г/);
});

test('сеть и отказ соседа — разные слова и разные подписи', () => {
  const net = Diag.describe('network');
  const http = Diag.describe('http-error');

  assert.notEqual(net.badge, http.badge);
  assert.notEqual(net.why, http.why);
});

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

test('частая политика задаётся параметром', () => {
  const r = recorder();
  const d = Diag.reporter({ subject: 'фид', every: 3, log: r.log });

  for (let i = 0; i < 7; i += 1) d.fail('malformed', null);

  assert.equal(r.errors.length, 3, `ожидали 3 строки, получили ${r.errors.length}`);
});

test('хост получает каждый отказ, даже когда лог молчит', () => {
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
  const r = recorder();
  const d = Diag.reporter({ subject: 'проба', log: r.log });

  const out = d.fail('http-error', 503);

  assert.equal(out.badge, Diag.CHANNEL['http-error'].badge);
  assert.equal(out.why, Diag.CHANNEL['http-error'].why);
  assert.equal(out.first, true);
});
