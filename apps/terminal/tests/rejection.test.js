'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadModule } = require('./helpers/load-module');

const Rejection = loadModule('rejection.js', 'Rejection');

const GATE_CODES = [
  'instrument_not_allowed',
  'tenant_instruments_unavailable',
  'tenant_instruments_unreadable',
];

test('у каждого отказа границы есть объяснение, а не код в лицо', () => {
  for (const code of GATE_CODES) {
    const t = Rejection.text(403, code);
    assert.ok(
      !t.includes(code),
      `«${code}» показывается человеку сырым кодом: ${t}`,
    );
    assert.ok(t.length > 20, `объяснение «${t}» слишком короткое, чтобы что-то объяснить`);
  }
});

test('три причины — три РАЗНЫХ текста, а не один экран', () => {
  const texts = GATE_CODES.map((c) => Rejection.text(503, c));
  assert.equal(
    new Set(texts).size,
    GATE_CODES.length,
    `одинаковые тексты отправляют чинить наугад: ${JSON.stringify(texts)}`,
  );
});

test('«списка нет» и «список непрочитаем» зовут разных людей', () => {
  const unavailable = Rejection.text(503, 'tenant_instruments_unavailable');
  const unreadable = Rejection.text(503, 'tenant_instruments_unreadable');

  assert.match(
    unavailable,
    /unreachable|did not arrive/i,
    `«списка нет» обязан говорить о недоступности источника: ${unavailable}`,
  );
  assert.match(
    unreadable,
    /CMS/,
    `«список непрочитаем» обязан звать владельца в CMS: ${unreadable}`,
  );
  assert.ok(
    !/CMS/.test(unavailable),
    `«списка нет» не должен звать в CMS — там чинить нечего: ${unavailable}`,
  );
});

test('неизвестный код не притворяется объяснением', () => {
  const t = Rejection.text(422, 'invariant_violated');
  assert.match(t, /does not recognise/i, `запасная ветка обязана признаться: ${t}`);
  assert.ok(t.includes('invariant_violated'), 'и назвать код, чтобы было с чем идти');
  assert.ok(t.includes('422'), 'и статус');
});

test('отсутствие сессии объясняется даже без кода', () => {
  assert.match(Rejection.text(401, undefined), /session/i);
});

test('код не затирается статусом', () => {
  assert.equal(
    Rejection.text(401, 'instrument_not_allowed'),
    Rejection.TEXT.instrument_not_allowed,
  );
});

const LEVEL_CODES = ['stop_loss_already_reached', 'take_profit_already_reached'];
const MIN_CODES = ['stop_loss_too_close', 'take_profit_too_close'];

test('уровень, уже достигнутый рынком, объяснён, а не показан кодом', () => {
  for (const code of LEVEL_CODES) {
    const t = Rejection.text(409, code);
    assert.ok(!t.includes(code), `«${code}» показывается сырым кодом: ${t}`);
    assert.ok(
      !/terminal does not know/i.test(t),
      `«${code}» уехал в запасную ветку — объяснения нет: ${t}`,
    );
  }
});

test('стоп и тейк — РАЗНЫЕ тексты: человек правит разные поля', () => {
  const [sl, tp] = LEVEL_CODES.map((c) => Rejection.text(409, c));
  assert.notEqual(sl, tp, 'один текст на оба случая отправит проверять оба поля');
  assert.match(sl, /stop loss/i, `текст обязан назвать поле: ${sl}`);
  assert.match(tp, /take profit/i, `текст обязан назвать поле: ${tp}`);
});

test('текст говорит, ЧТО сделать, а не только что отказано', () => {
  for (const code of LEVEL_CODES) {
    const t = Rejection.text(409, code);
    assert.match(t, /move it/i, `нет действия: ${t}`);
    assert.match(t, /buy/i, `не названа сторона для покупки: ${t}`);
    assert.match(t, /sell/i, `не названа сторона для продажи: ${t}`);
  }
});

const OPEN_CODES = [
  'demo_start_balance_not_set',
  'demo_start_balance_unavailable',
  'identity_store_unavailable',
];

test('отказ в заведении счёта объяснён, а не показан кодом', () => {
  for (const code of OPEN_CODES) {
    const t = Rejection.text(409, code);
    assert.ok(!t.includes(code), `«${code}» показывается сырым кодом: ${t}`);
    assert.ok(
      !/terminal does not know/i.test(t),
      `«${code}» уехал в запасную ветку: ${t}`,
    );
  }
});

test('три причины — три РАЗНЫХ текста: чинят их разные люди', () => {
  const texts = OPEN_CODES.map((c) => Rejection.text(409, c));
  assert.equal(new Set(texts).size, texts.length, `тексты совпадают: ${texts.join(' | ')}`);
  assert.match(Rejection.text(409, 'demo_start_balance_not_set'), /admin panel/i);
});

test('отказ не обвиняет человека в его же запросе', () => {
  for (const code of OPEN_CODES) {
    assert.match(
      Rejection.text(409, code),
      /nothing is wrong with your request|your request was fine/i,
      `${code}: человек подумает, что виноват он: ${Rejection.text(409, code)}`,
    );
  }
});

test('заморозка объяснена: что запрещено и что ещё можно', () => {
  const t = Rejection.text(403, 'account_frozen');
  assert.ok(!t.includes('account_frozen'), `сырой код: ${t}`);
  assert.match(t, /operator/i, `не сказано, кто заблокировал: ${t}`);
  assert.match(t, /close/i, `не сказано, что закрыть можно: ${t}`);
});

test('стоп ближе минимума площадки объяснён словами, и поле названо', () => {
  const [sl, tp] = MIN_CODES.map((c) => Rejection.text(422, c));
  for (const [code, t] of [[MIN_CODES[0], sl], [MIN_CODES[1], tp]]) {
    assert.ok(!t.includes(code), `«${code}» показывается сырым кодом: ${t}`);
    assert.ok(!/does not recognise/i.test(t), `«${code}» уехал в запасную ветку: ${t}`);
  }
  assert.match(sl, /stop loss/i);
  assert.match(tp, /take profit/i);
  assert.notEqual(sl, tp);
});

test('закрытый рынок, приостановка и «только закрытие» — три разных объяснения, не код', () => {
  const codes = ['market_closed', 'market_halted', 'instrument_close_only'];
  const texts = codes.map((c) => Rejection.text(409, c));
  texts.forEach((t, i) => assert.ok(!t.includes(codes[i]) && t.length > 20, `«${codes[i]}» объяснён: ${t}`));
  assert.equal(new Set(texts).size, 3);
  assert.match(texts[0], /limit order/i, 'при закрытом рынке подсказываем отложенный ордер');
  assert.match(texts[2], /close existing positions/i, 'closeOnly не запрещает закрывать');
});
