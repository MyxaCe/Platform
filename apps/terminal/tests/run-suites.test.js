'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const path = require('node:path');

const { classify, runSuite, exitCodeFor } = require('./run-suites');

const NL = '\n';

function summary(tests, fail) {
  return [
    'ℹ tests ' + tests,
    'ℹ suites 0',
    'ℹ pass ' + (tests - fail),
    'ℹ fail ' + fail,
    'ℹ cancelled 0',
  ].join('\n');
}

test('сводка без отказов и нулевой код — пройден', () => {
  const r = classify(summary(17, 0), 0, null);
  assert.equal(r.outcome, 'пройден');
  assert.equal(r.count, 17);
});

test('сводка с отказами — отказы, и их число названо', () => {
  const r = classify(summary(17, 3), 1, null);
  assert.equal(r.outcome, 'отказы');
  assert.equal(r.failed, 3);
  assert.equal(r.count, 17);
});

test('сводки нет — НЕ ОТРАБОТАЛ, и это не отказ теста', () => {
  const r = classify('что-то вывалилось и оборвалось', 1, null);
  assert.equal(r.outcome, 'НЕ ОТРАБОТАЛ');
  assert.equal(r.code, 1);
  assert.match(r.why, /сводки/);
});

test('пустой вывод — тоже НЕ ОТРАБОТАЛ, а не «нечего проверять»', () => {
  const r = classify('', null, 'SIGKILL');
  assert.equal(r.outcome, 'НЕ ОТРАБОТАЛ');
  assert.equal(r.signal, 'SIGKILL');
});

test('сводка есть, отказов нет, но код ненулевой — не успех', () => {
  const r = classify(summary(17, 0), 7, null);
  assert.equal(r.outcome, 'отказы');
});

test('обрыв на полуслове не считается сводкой', () => {
  const r = classify('ℹ tests 17\nℹ suites 0\n', 1, null);
  assert.equal(r.outcome, 'НЕ ОТРАБОТАЛ');
});

test('вложенный прогон не подменяет охват своим числом', () => {
  const nested = [summary(1, 0), '', summary(177, 0)].join(NL);
  const r = classify(nested, 0, null);

  assert.equal(r.outcome, 'пройден');
  assert.equal(r.count, 177, 'взять надо последнюю сводку, свою, а не первую');
});

test('вложенные отказы не выдаются за свои', () => {
  const nested = [summary(1, 1), '', summary(177, 0)].join(NL);
  assert.equal(classify(nested, 0, null).outcome, 'пройден');

  const inverse = [summary(1, 0), '', summary(177, 2)].join(NL);
  assert.equal(classify(inverse, 1, null).outcome, 'отказы');
  assert.equal(classify(inverse, 1, null).failed, 2);
});

test('у «не отработал» и «отказы» разные коды возврата', () => {
  const dead = [{ outcome: 'НЕ ОТРАБОТАЛ' }];
  const failed = [{ outcome: 'отказы' }];
  const ok = [{ outcome: 'пройден' }];

  assert.equal(exitCodeFor(ok), 0);
  assert.equal(exitCodeFor(failed), 1);
  assert.equal(exitCodeFor(dead), 2);
  assert.notEqual(exitCodeFor(dead), exitCodeFor(failed));
});

test('неизвестный результат перевешивает известный отказ', () => {
  assert.equal(exitCodeFor([{ outcome: 'отказы' }, { outcome: 'НЕ ОТРАБОТАЛ' }]), 2);
  assert.equal(exitCodeFor([{ outcome: 'НЕ ОТРАБОТАЛ' }, { outcome: 'отказы' }]), 2);
});

test('формат вывода закрепляет сам запускатель, а не записи наборов', () => {
  const { REPORTER, SUITES } = require('./run-suites');

  assert.ok(
    REPORTER.some((a) => String(a).startsWith('--test-reporter=')),
    'запускатель обязан закреплять формат',
  );
  for (const s of SUITES) {
    assert.ok(
      !s.args.some((a) => String(a).startsWith('--test-reporter')),
      `запись «${s.name}» дублирует решение о формате — оно должно быть одно`,
    );
  }
});

test('набор в трубе отчитывается так же, как в терминале', async () => {
  const path = require('node:path');
  const one = path.join(__dirname, 'fixtures', 'one-passing.test.js');
  const r = await runSuite({ name: 'в трубе', args: ['--test'], files: [one] });

  assert.equal(r.outcome, 'пройден', 'в трубе исход обязан читаться так же');
  assert.equal(r.count, 1);
});

test('набор, который не запустился, назван НЕ ОТРАБОТАВШИМ (живой процесс)', async () => {
  const one = path.join(__dirname, 'fixtures', 'one-passing.test.js');
  const r = await runSuite({ name: 'заведомо негодный', args: ['--test', '--негодный-флаг'], files: [one] });

  assert.equal(r.outcome, 'НЕ ОТРАБОТАЛ');
  assert.notEqual(r.code, 0, 'код возврата обязан быть ненулевым');
  assert.match(String(r.stderr), /bad option|негодный/i, 'поток ошибок обязан дойти до отчёта');
});

test('здоровый набор на живом процессе назван пройденным', async () => {
  const one = path.join(__dirname, 'fixtures', 'one-passing.test.js');
  const r = await runSuite({ name: 'здоровый', args: ['--test'], files: [one] });

  assert.equal(r.outcome, 'пройден');
  assert.equal(r.failed, 0);
  assert.ok(r.count >= 1);
});

test('каталог без тестов — НЕ ОТРАБОТАЛ, а не «ноль отказов»', async () => {
  const r = await runSuite({ name: 'пустой', dir: 'tests/нет-такого-каталога', args: ['--test'] });

  assert.equal(r.outcome, 'НЕ ОТРАБОТАЛ');
  assert.match(r.why, /не найдено ни одного файла/);
});

test('наборы находят свои файлы, и их не ноль', () => {
  const { SUITES, filesIn } = require('./run-suites');

  for (const s of SUITES) {
    const found = filesIn(s.dir);
    assert.ok(found.length > 0, `набор «${s.name}»: в ${s.dir} не найдено файлов`);
    assert.ok(found.every((f) => f.endsWith('.test.js')), `набор «${s.name}»: попало лишнее`);
  }
});
