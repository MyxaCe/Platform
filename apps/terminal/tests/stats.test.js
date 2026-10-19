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

test('мутация «исключение источника глушится молча» здесь непрогоняема', () => {
  // `refresh` ловит любое исключение и выходит без следа: сетевой блип и
  // ошибка формы ответа неразличимы, и наружу ни то ни другое не выходит.
  // Покрасить это тестом нечем — наблюдаемого следа у подавленной диагностики
  // по определению нет. Это находка, а не свойство теста; вынесена в отчёт
  // штабу отдельной строкой.
  assert.ok(true);
});
