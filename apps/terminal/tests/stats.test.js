'use strict';
// ============================================================================
// Полоса статистики актива: «период → изменение %» (TRM-05, ADR-015).
//
// Главное свойство здесь одно и оно не про арифметику: «сервер не смог
// посчитать» обязано отличаться от «рынок стоял». Раньше сервер слал в этом
// случае ноль, и ноль рисовался как настоящее нулевое изменение (BUG-011) —
// ровно тот класс, что `unwrap_or(0.0)` в котировках: отказ, надевший
// правдоподобное значение.
// ============================================================================
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadWidget } = require('./helpers/load-widget');

const settle = () => new Promise((r) => setImmediate(r));

function setup() {
  return loadWidget(['stats.js']);
}

function mount(env, over) {
  const calls = { fetch: [] };
  const ctx = Object.assign(
    {
      instrument: () => 1,
      fetch: async (id) => { calls.fetch.push(id); return [{ tf: 60, change: 1.5 }]; },
      labels: { 60: '1m', 300: '5m' },
      interval: 0,
    },
    over || {},
  );
  return { widget: env.window.AssetStats.mount(env.root, ctx), calls };
}

const cells = (root) => [...root.querySelectorAll('.stat')].map((c) => ({
  label: c.querySelector('.stat__l').textContent,
  value: c.querySelector('.stat__v').textContent,
  cls: c.querySelector('.stat__v').className,
}));

test('изменение показывается со знаком и подписью периода', async (t) => {
  const env = setup();
  t.after(env.cleanup);
  mount(env, { fetch: async () => [{ tf: 60, change: 1.5 }, { tf: 300, change: -0.25 }] });
  await settle();

  assert.deepEqual(cells(env.root), [
    { label: '1m', value: '+1.50%', cls: 'stat__v up' },
    { label: '5m', value: '-0.25%', cls: 'stat__v down' },
  ]);
});

test('период без подписи показывается своим числом, а не пропадает', async (t) => {
  const env = setup();
  t.after(env.cleanup);
  mount(env, { labels: { 60: '1m' }, fetch: async () => [{ tf: 604800, change: 3 }] });
  await settle();

  assert.equal(cells(env.root)[0].label, '604800');
});

test('не посчитанное изменение — прочерк, а не ноль процентов (BUG-011)', async (t) => {
  // Ноль здесь правдоподобен: по «0.00%» человек решит, что рынок стоял, а по
  // прочерку пойдёт разбираться.
  const env = setup();
  t.after(env.cleanup);
  mount(env, { fetch: async () => [{ tf: 60, change: null }, { tf: 300, change: 0 }] });
  await settle();

  const c = cells(env.root);
  assert.equal(c[0].value, '—', 'не посчитали');
  assert.equal(c[0].cls, 'stat__v', 'не красим ни вверх, ни вниз — направления нет');
  assert.equal(c[1].value, '+0.00%', 'настоящий ноль остаётся нулём');
  assert.equal(c[1].cls, 'stat__v up');
});

test('отказ источника не стирает последние числа', async (t) => {
  // Пустая полоса означала бы «изменений нет». Прежние числа устарели на один
  // тик, и это честнее пустоты.
  const env = setup();
  t.after(env.cleanup);

  let mode = 'ok';
  const { widget } = mount(env, {
    fetch: async () => {
      if (mode === 'throw') throw new Error('сеть');
      if (mode === 'junk') return null;
      return [{ tf: 60, change: 2 }];
    },
  });
  await settle();
  assert.equal(cells(env.root).length, 1);

  mode = 'throw';
  await widget.refresh();
  await settle();
  assert.equal(cells(env.root)[0].value, '+2.00%', 'после исключения числа на месте');

  mode = 'junk';
  await widget.refresh();
  await settle();
  assert.equal(cells(env.root)[0].value, '+2.00%', 'после негодного ответа тоже');
});

test('инструмент не выбран — источник не дёргается вовсе', async (t) => {
  const env = setup();
  t.after(env.cleanup);
  const { calls } = mount(env, { instrument: () => null });
  await settle();

  assert.deepEqual(calls.fetch, [], 'запроса без инструмента быть не должно');
});

test('статистика запрашивается для текущего инструмента, а не для первого', async (t) => {
  const env = setup();
  t.after(env.cleanup);

  let current = 1;
  const { widget, calls } = mount(env, { instrument: () => current });
  await settle();

  current = 42;
  await widget.refresh();
  await settle();

  assert.deepEqual(calls.fetch, [1, 42]);
});

test('destroy останавливает опрос и очищает полосу', async (t) => {
  const env = setup();
  t.after(env.cleanup);

  let hits = 0;
  const { widget } = mount(env, {
    interval: 5,
    fetch: async () => { hits += 1; return [{ tf: 60, change: 1 }]; },
  });
  await settle();
  await new Promise((r) => env.window.setTimeout(r, 30));
  const during = hits;
  assert.ok(during > 1, `опрос обязан идти, вызовов ${during}`);

  widget.destroy();
  assert.equal(env.root.innerHTML, '');

  await new Promise((r) => env.window.setTimeout(r, 30));
  assert.equal(hits, during, 'после destroy источник не дёргается');
});

// ---- Отказ виден как отказ (TRM-16) ---------------------------------------

test('источник не ответил — на полосе метка, а не вид свежих чисел', async (t) => {
  // Было `catch { return; }`: полоса продолжала показывать старые числа как
  // свежие. Молчаливый отказ надевал вид работающего виджета.
  const env = setup();
  t.after(env.cleanup);

  let mode = 'ok';
  const { widget } = mount(env, {
    fetch: async () => {
      if (mode === 'throw') throw new Error('сеть');
      return [{ tf: 60, change: 2 }];
    },
  });
  await settle();
  assert.equal(env.root.getAttribute('data-stale'), null, 'пока всё хорошо — метки нет');

  mode = 'throw';
  await widget.refresh();
  await settle();

  assert.equal(env.root.getAttribute('data-stale'), 'unreachable');
  assert.match(env.root.getAttribute('data-stale-label'), /НЕТ СВЯЗИ/);
  assert.match(env.root.title, /не ответил/);
  assert.equal(cells(env.root)[0].value, '+2.00%', 'числа остаются: они были верны');
});

test('ответ не той формы — своя метка, а не та же, что у обрыва связи', async (t) => {
  // Сетевой блип проходит сам, ошибка формы — нет. Схлопывать их в один отказ
  // значит отправлять чинить наугад.
  const env = setup();
  t.after(env.cleanup);

  let mode = 'ok';
  const { widget } = mount(env, {
    fetch: async () => (mode === 'junk' ? { периоды: [] } : [{ tf: 60, change: 2 }]),
  });
  await settle();

  mode = 'junk';
  await widget.refresh();
  await settle();

  assert.equal(env.root.getAttribute('data-stale'), 'malformed');
  assert.match(env.root.getAttribute('data-stale-label'), /ОТВЕТ НЕ ТОТ/);
  assert.match(env.root.title, /ответил не тем/);
});

test('две причины отказа различимы между собой', async (t) => {
  // Главное свойство починки: не «след есть», а «след называет, какой именно
  // отказ». Без этой проверки обе причины могли бы схлопнуться в одну метку, и
  // тест на наличие метки остался бы зелёным.
  const env = setup();
  t.after(env.cleanup);

  let mode = 'throw';
  const { widget } = mount(env, {
    fetch: async () => {
      if (mode === 'throw') throw new Error('сеть');
      if (mode === 'junk') return null;
      return [{ tf: 60, change: 1 }];
    },
  });
  await settle();
  const first = env.root.getAttribute('data-stale');

  mode = 'junk';
  await widget.refresh();
  await settle();
  const second = env.root.getAttribute('data-stale');

  assert.notEqual(first, second, `обе причины дали одну метку «${first}»`);
});

test('отказ уходит наружу хосту, а не только на экран', async (t) => {
  // У хоста свой индикатор состояния; виджет обязан дать ему повод, а не
  // прятать отказ внутри себя.
  const env = setup();
  t.after(env.cleanup);

  const seen = [];
  let mode = 'ok';
  const { widget } = mount(env, {
    onError: (e) => seen.push(e.kind),
    fetch: async () => {
      if (mode === 'throw') throw new Error('сеть');
      return [{ tf: 60, change: 1 }];
    },
  });
  await settle();
  assert.deepEqual(seen, [], 'на успехе никого не будим');

  mode = 'throw';
  await widget.refresh();
  await widget.refresh();
  await settle();

  assert.deepEqual(seen, ['unreachable', 'unreachable'], 'каждый неудачный тик — наружу');
});

test('после восстановления метка снимается', async (t) => {
  // Застрявшая метка — такая же ложь, как её отсутствие: полоса обновляется, а
  // на ней написано, что нет.
  const env = setup();
  t.after(env.cleanup);

  let mode = 'throw';
  const { widget } = mount(env, {
    fetch: async () => {
      if (mode === 'throw') throw new Error('сеть');
      return [{ tf: 60, change: 3 }];
    },
  });
  await settle();
  assert.equal(env.root.getAttribute('data-stale'), 'unreachable');

  mode = 'ok';
  await widget.refresh();
  await settle();

  assert.equal(env.root.getAttribute('data-stale'), null);
  assert.equal(env.root.getAttribute('data-stale-label'), null);
  assert.equal(env.root.title, '');
  assert.equal(cells(env.root)[0].value, '+3.00%');
});

test('поток одинаковых отказов не заливает консоль', async (t) => {
  // Шумную диагностику чинят в тот же день, но сто одинаковых строк читать
  // перестанут так же быстро, как молчание. Пишем на смене состояния.
  const env = setup();
  t.after(env.cleanup);

  const errs = [];
  const real = env.window.console.error;
  env.window.console.error = (...a) => errs.push(a);
  console.error = env.window.console.error;
  t.after(() => { env.window.console.error = real; console.error = real; });

  const { widget } = mount(env, { fetch: async () => { throw new Error('сеть'); } });
  await settle();
  for (let i = 0; i < 5; i += 1) { await widget.refresh(); }
  await settle();

  assert.equal(errs.length, 1, `одна строка на серию, а не ${errs.length}`);
});
