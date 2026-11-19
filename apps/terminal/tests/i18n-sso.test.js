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
const LANGS = ['ru', 'en', 'de', 'fr', 'es'];

function i18n(t, url) {
  const env = loadWidget(['rejection.js', 'i18n.js'], { url });
  t.after(env.cleanup);
  return /** @type {any} */ (env.window).I18n;
}

/** @param {string} s */
const holes = (s) => [...s.matchAll(/\{(\w+)\}/g)].map((m) => m[1]).sort().join(',');

test('язык по умолчанию — RU; язык кабинета из ссылки главнее сохранённого', (t) => {
  const I = i18n(t);
  assert.equal(I.detect({}), 'ru', 'ничего не передано — русский');
  assert.equal(I.detect({ saved: 'en' }), 'en', 'выбор человека помнится');
  assert.equal(I.detect({ query: '?lang=ru', saved: 'en' }), 'ru', 'кабинет передал язык — он главнее');
  assert.equal(I.detect({ hash: '#handoff=x&lang=en' }), 'en', 'язык во фрагменте входной ссылки');
  assert.equal(I.detect({ query: '?lang=de' }), 'de');
  assert.equal(I.detect({ query: '?lang=xx' }), 'ru', 'неизвестный язык — не принимаем');
  assert.equal(I.detect({ config: 'en-US' }), 'en');
});

test('пять словарей: одинаковые ключи, непустые строки, одинаковые плейсхолдеры', (t) => {
  const I = i18n(t);
  assert.deepEqual([...I.LANGS], LANGS);
  const keys = Object.keys(I.DICTS.en).sort();
  assert.ok(keys.length > 200);
  for (const l of LANGS) {
    const tb = I.DICTS[l];
    assert.deepEqual(Object.keys(tb).sort(), keys, `${l}: набор ключей`);
    for (const k of keys) {
      assert.ok(typeof tb[k] === 'string' && tb[k].trim(), `${l}.${k}: пустой перевод`);
      assert.equal(holes(tb[k]), holes(I.DICTS.en[k]), `${l}.${k}: плейсхолдеры`);
    }
  }
  for (const [k, pair] of Object.entries(I.DICT)) {
    assert.deepEqual([...pair], [I.DICTS.en[k], I.DICTS.ru[k]], `${k}: DICT — пара [en, ru]`);
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
  assert.equal(I.rejection(403, 'instrument_not_allowed'), 'This instrument is not available on this site');
  assert.equal(I.widgetT('нет.такого', 'Fallback'), 'Fallback', 'виджет остаётся при своём английском');
  assert.equal(I.widgetT('нет.такого', 'Hi {n}', { n: 2 }), 'Hi 2');
});

test('виджет получает de/fr/es по тем же ключам, отказы — на языке интерфейса', (t) => {
  const I = i18n(t, 'http://localhost:8888/?lang=de');
  assert.equal(I.lang(), 'de');
  assert.equal(I.widgetT('ob.title', 'Market depth'), 'Markttiefe');
  assert.equal(I.t('acct.balance'), 'Kontostand');
  assert.match(I.rejection(409, 'stop_loss_already_reached'), /^Ihr Stop-Loss/);
  assert.equal(I.rejection(401), 'Keine Sitzung im Kundenbereich — Handel nicht verfügbar');
  assert.match(I.rejection(503, 'neu_code'), /Abgelehnt \(503\).*„neu_code“/);
  assert.equal(I.rejection(500), 'Abgelehnt (500)');
  I.setSite({ name: 'X', languages: ['ru', 'en', 'de', 'fr', 'es'], defaultLanguage: 'ru' });
  I.setLang('fr');
  assert.equal(I.t('acct.equity'), 'Fonds propres');
  assert.match(I.rejection(422, 'stop_loss_too_close'), /^Votre Stop Loss/);
  I.setLang('es');
  assert.equal(I.t('acct.leverage'), 'Apalancamiento');
  assert.match(I.rejection(403, 'instrument_not_allowed'), /no está disponible/);
});

test('у каждого кода отказа из rejection.js есть текст на ru, de, fr, es', (t) => {
  const env = loadWidget(['rejection.js', 'i18n.js']);
  t.after(env.cleanup);
  const w = /** @type {any} */ (env.window);
  const codes = Object.keys(w.Rejection.TEXT).sort();
  for (const l of ['ru', 'de', 'fr', 'es']) {
    assert.deepEqual(Object.keys(w.I18n.REJECT[l]).sort(), codes, `${l}: набор кодов`);
    for (const c of codes) assert.ok(w.I18n.REJECT[l][c].trim(), `${l}: нет перевода отказа ${c}`);
  }
  assert.equal(w.I18n.REJECT_RU, w.I18n.REJECT.ru);
});

test('языки сайта из brand: поля, вложенная форма v2, мусор, язык по умолчанию', (t) => {
  const I = i18n(t);
  assert.equal(I.siteLanguages(null), null);
  assert.equal(I.siteLanguages({ name: 'X' }), null, 'полей нет — старая CMS');
  assert.equal(I.siteLanguages({ languages: [] }), null);
  assert.equal(I.siteLanguages({ languages: ['xx'] }), null);
  const flat = I.siteLanguages({ languages: ['de', 'EN', 'de', 'zz'], defaultLanguage: 'en' });
  assert.deepEqual(JSON.parse(JSON.stringify(flat)), { languages: ['de', 'en'], defaultLanguage: 'en' });
  const v2 = I.siteLanguages({ site: { slug: 's', name: 'S', languages: ['fr', 'es'], defaultLanguage: 'ru' }, brand: {} });
  assert.deepEqual(JSON.parse(JSON.stringify(v2)), { languages: ['fr', 'es'], defaultLanguage: 'fr' }, 'язык по умолчанию вне списка — первый из списка');
});

test('выбор языка: вне списка сайта — язык по умолчанию; brand без полей — ru и en, прочее — en', (t) => {
  const I = i18n(t);
  const site = { languages: ['ru', 'en', 'de'], defaultLanguage: 'ru' };
  assert.equal(I.resolve('de', site), 'de');
  assert.equal(I.resolve('fr', site), 'ru');
  assert.equal(I.resolve(null, { languages: ['es', 'en'], defaultLanguage: 'es' }), 'es');
  assert.equal(I.resolve('ru', null), 'ru');
  assert.equal(I.resolve('en', null), 'en');
  assert.equal(I.resolve('de', null), 'en');
  assert.equal(I.resolve(null, null), 'ru');
});

test('brand пришёл: язык из ссылки проверяется по списку сайта, переключатель — только языки сайта', (t) => {
  const I = i18n(t, 'http://localhost:8888/?lang=fr');
  const seen = [];
  I.onChange((/** @type {string} */ l) => seen.push(l));
  assert.equal(I.lang(), 'fr', 'до ответа brand — язык кабинета');
  I.setSite({ name: 'X', languages: ['ru', 'de'], defaultLanguage: 'de' });
  assert.equal(I.lang(), 'de', 'fr сайтом не включён — язык сайта по умолчанию');
  assert.deepEqual([...I.available()], ['ru', 'de']);
  I.setLang('es');
  assert.equal(I.lang(), 'de', 'язык вне списка сайта не включается');
  I.setLang('ru');
  assert.equal(I.lang(), 'ru');
  assert.deepEqual(seen, ['de', 'ru']);
});

test('brand недоступен: de из ссылки заменяется на en, ru и en остаются', (t) => {
  const de = i18n(t, 'http://localhost:8888/?lang=de');
  de.setSite(null);
  assert.equal(de.lang(), 'en');
  assert.deepEqual([...de.available()], ['ru', 'en']);
  const ru = i18n(t, 'http://localhost:8888/?lang=ru');
  ru.setSite({ name: 'old cms' });
  assert.equal(ru.lang(), 'ru');
});

test('числа, деньги и проценты — через Intl с тегом языка', (t) => {
  const I = i18n(t, 'http://localhost:8888/?lang=en');
  const nb = (/** @type {string} */ s) => s.replace(/[  ]/g, ' ');
  const cases = {
    en: ['en-US', '65,000.50', '-$1,234.56', '80.5%', '+1.50%'],
    ru: ['ru-RU', '65 000,50', '-1 234,56 $', '80,5 %', '+1,50 %'],
    de: ['de-DE', '65.000,50', '-1.234,56 $', '80,5 %', '+1,50 %'],
    fr: ['fr-FR', '65 000,50', '-1 234,56 $', '80,5 %', '+1,50 %'],
    es: ['es-ES', '65.000,50', '-1234,56 $', '80,5 %', '+1,50 %'],
  };
  I.setSite({ languages: LANGS, defaultLanguage: 'en' });
  for (const [l, [tag, n, m, p, c]] of Object.entries(cases)) {
    I.setLang(l);
    assert.equal(I.locale(), tag);
    assert.equal(nb(I.num(65000.5, 2)), n, `${l}: число`);
    assert.equal(nb(I.money(-123456)), m, `${l}: деньги`);
    assert.equal(nb(I.pct(80.5, 0, 2)), p, `${l}: процент`);
    assert.equal(nb(I.change(1.5)), c, `${l}: изменение`);
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
