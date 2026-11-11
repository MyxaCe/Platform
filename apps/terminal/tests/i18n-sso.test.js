'use strict';
// ============================================================================
// Локализация (i18n.js) и граница входа верхним уровнем (sso.js), 10.11.
//
// Словарь проверяется на ПОЛНОТУ, а не на отдельные строки: недостающий ключ
// печатается в интерфейсе как сам ключ, и находит его тогда человек на показе.
// Граница входа — чистые функции: в них вся безопасность (открытый редирект,
// подмена кабинета), и проверять её через браузер значило бы проверять браузер.
// ============================================================================
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const { loadWidget } = require('./helpers/load-widget');

const PUBLIC_DIR = path.join(__dirname, '..', 'public');

function i18n(t) {
  const env = loadWidget(['i18n.js']);
  t.after(env.cleanup);
  return /** @type {any} */ (env.window).I18n;
}

test('язык по умолчанию — RU; язык кабинета из ссылки главнее сохранённого', (t) => {
  const I = i18n(t);
  assert.equal(I.detect({}), 'ru', 'ничего не передано — русский');
  assert.equal(I.detect({ saved: 'en' }), 'en', 'выбор человека помнится');
  assert.equal(I.detect({ query: '?lang=ru', saved: 'en' }), 'ru', 'кабинет передал язык — он главнее');
  assert.equal(I.detect({ hash: '#handoff=x&lang=en' }), 'en', 'язык во фрагменте входной ссылки');
  assert.equal(I.detect({ query: '?lang=de' }), 'ru', 'неизвестный язык — не принимаем');
  assert.equal(I.detect({ config: 'en-US' }), 'en');
});

test('в словаре у каждого ключа оба языка и непустые', (t) => {
  const I = i18n(t);
  for (const [k, pair] of Object.entries(I.DICT)) {
    assert.equal(pair.length, 2, `${k}: нужны en и ru`);
    assert.ok(pair[0] && pair[1], `${k}: пустой перевод`);
  }
});

test('каждый data-i18n из разметки есть в словаре', (t) => {
  const I = i18n(t);
  for (const file of ['index.html', 'logout.html']) {
    const html = fs.readFileSync(path.join(PUBLIC_DIR, file), 'utf8');
    const keys = [...html.matchAll(/data-i18n(?:-title|-tip|-ph)?="([^"]+)"/g)].map((m) => m[1]);
    assert.ok(keys.length >= 4, `${file}: разметка размечена`);
    for (const k of keys) assert.ok(I.has(k), `${file}: ключа «${k}» нет в словаре`);
  }
});

test('подстановки и отказы границы переводятся, неизвестный код называет себя', (t) => {
  const I = i18n(t);
  assert.match(I.t('mc.call', { lvl: 80, call: 100, stop: 50 }), /80%.*100%.*50%/);
  assert.equal(I.rejection(403, 'instrument_not_allowed'), 'Этот инструмент недоступен на этом сайте');
  assert.match(I.rejection(503, 'что-то_новое'), /не знает причину «что-то_новое»/);
  I.setLang('en');
  assert.equal(I.lang(), 'en');
  assert.equal(I.t('acct.balance'), 'Balance');
  assert.equal(I.widgetT('нет.такого', 'Fallback'), 'Fallback', 'виджет остаётся при своём английском');
});

test('у каждого кода отказа из rejection.js есть русский текст', (t) => {
  const env = loadWidget(['rejection.js', 'i18n.js']);
  t.after(env.cleanup);
  const w = /** @type {any} */ (env.window);
  for (const code of Object.keys(w.Rejection.TEXT)) {
    assert.ok(w.I18n.REJECT_RU[code], `нет перевода отказа ${code}`);
  }
});

// ---- Вход верхним уровнем --------------------------------------------------

function sso(t) {
  const env = loadWidget(['sso.js']);
  t.after(env.cleanup);
  return /** @type {any} */ (env.window).Sso;
}

test('фрагмент: токен, род счёта, чужие ключи и мусор', (t) => {
  const S = sso(t);
  const f = S.parseFragment('#handoff=eyJ.abc.def&account=real&lang=en');
  assert.equal(f.handoff, 'eyJ.abc.def');
  assert.equal(f.account, 'real');
  assert.equal(f.lang, 'en');
  assert.equal(f.any, true);
  assert.equal(S.parseFragment('#account=admin').account, null, 'род счёта — только demo|real');
  assert.equal(S.parseFragment('#handoff=').handoff, null, 'пустой токен — не токен');
  assert.equal(S.parseFragment('#chart=1').any, false, 'чужой фрагмент не стираем');
  assert.equal(S.parseFragment('').handoff, null);
});

test('кабинет — только из белого списка: подмена из ссылки или Referer не проходит', (t) => {
  const S = sso(t);
  const allowed = ['https://cab-a.example', 'https://cab-b.example'];
  assert.equal(S.pickCabinet(['https://cab-b.example/dashboard'], allowed), 'https://cab-b.example');
  assert.equal(S.pickCabinet(['https://evil.example', null], allowed), 'https://cab-a.example', 'чужой — нет, берём первый из списка');
  assert.equal(S.pickCabinet(['https://cab-a.example.evil.example'], allowed), 'https://cab-a.example', 'совпадение origin точное, не префикс');
  assert.equal(S.pickCabinet(['javascript:alert(1)'], allowed), 'https://cab-a.example');
  assert.equal(S.pickCabinet(['https://cab-a.example'], []), null, 'списка нет — кабинета нет');
});

test('адрес продления: маршрут кабинета, род счёта и адрес возврата закодированы', (t) => {
  const S = sso(t);
  const u = new URL(S.launchUrl('https://cab.example', 'demo', 'https://terminal.example/?site=apex-ru'));
  assert.equal(u.origin + u.pathname, 'https://cab.example/api/sso/launch');
  assert.equal(u.searchParams.get('account'), 'demo');
  assert.equal(u.searchParams.get('return'), 'https://terminal.example/?site=apex-ru');
  assert.equal(new URL(S.launchUrl('https://cab.example', null, 'x')).searchParams.has('account'), false);
});
