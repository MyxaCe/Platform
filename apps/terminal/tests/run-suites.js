'use strict';
const { spawn } = require('node:child_process');
const fs = require('node:fs');
const path = require('node:path');

const REPORTER = ['--test-reporter=spec', '--test-reporter-destination=stdout'];

function filesIn(dir, { recurse = false } = {}) {
  const full = path.join(__dirname, '..', dir);
  if (!fs.existsSync(full)) return [];
  return fs.readdirSync(full, { withFileTypes: true })
    .flatMap((e) => {
      const p = path.join(full, e.name);
      if (e.isDirectory()) return recurse ? filesIn(path.join(dir, e.name), { recurse }) : [];
      return e.isFile() && e.name.endsWith('.test.js') ? [p] : [];
    })
    .sort();
}

const SUITES = [
  { name: 'модульные', dir: 'tests', args: ['--test'] },
  { name: 'браузерные', dir: 'tests/browser', args: ['--test', '--test-concurrency=1'] },
];

const TESTS_RE = /^ℹ tests (\d+)/gm;
const FAIL_RE = /^ℹ fail (\d+)/gm;

function lastMatch(output, re) {
  re.lastIndex = 0;
  let found = null;
  let m = re.exec(output);
  while (m) {
    found = m;
    m = re.exec(output);
  }
  return found;
}

function classify(output, code, signal) {
  const tests = lastMatch(output, TESTS_RE);
  const fails = lastMatch(output, FAIL_RE);

  if (!tests || !fails) {
    return {
      outcome: 'НЕ ОТРАБОТАЛ',
      code,
      signal,
      why: 'итоговой сводки в выводе нет — процесс не дошёл до конца',
    };
  }

  const failed = Number(fails[1]);
  return {
    outcome: failed > 0 || code !== 0 ? 'отказы' : 'пройден',
    count: Number(tests[1]),
    failed,
    code,
    signal,
  };
}

function runSuite(suite) {
  return new Promise((resolve) => {
    const env = Object.assign({}, process.env);
    delete env.NODE_TEST_CONTEXT;

    const files = suite.files || filesIn(suite.dir);
    if (!files.length) {
      resolve({
        suite, stderr: '', outcome: 'НЕ ОТРАБОТАЛ', code: null, signal: null,
        why: `в каталоге ${suite.dir} не найдено ни одного файла *.test.js — ` +
             'запускать нечего, и это не «ноль отказов»',
      });
      return;
    }

    const child = spawn(process.execPath, [...suite.args, ...REPORTER, ...files], { env });

    let stdout = '';
    let stderr = '';
    child.stdout.on('data', (c) => { stdout += c; process.stdout.write(c); });
    child.stderr.on('data', (c) => { stderr += c; process.stderr.write(c); });

    child.on('error', (e) => {
      resolve({ suite, outcome: 'НЕ ОТРАБОТАЛ', code: null, signal: null, why: String(e), stderr });
    });

    child.on('close', (code, signal) => {
      resolve(Object.assign({ suite, stderr }, classify(stdout + stderr, code, signal)));
    });
  });
}

function exitCodeFor(results) {
  let worst = 0;
  for (const r of results) {
    if (r.outcome === 'НЕ ОТРАБОТАЛ') worst = Math.max(worst, 2);
    else if (r.outcome === 'отказы') worst = Math.max(worst, 1);
  }
  return worst;
}

async function main() {
  const results = [];
  for (const suite of SUITES) {
    results.push(await runSuite(suite));
  }

  process.stdout.write('\n==================== исход наборов ====================\n');

  for (const r of results) {
    if (r.outcome === 'НЕ ОТРАБОТАЛ') {
      process.stdout.write(
        `  ${r.suite.name}: НЕ ОТРАБОТАЛ — ${r.why}\n` +
        `      код выхода: ${r.code}, сигнал: ${r.signal ?? 'нет'}\n` +
        '      Результат набора НЕИЗВЕСТЕН: это не «тесты прошли» и не «тест упал».\n',
      );
      const tail = String(r.stderr || '').trim().split('\n').slice(-12);
      if (tail.length && tail[0]) {
        process.stdout.write('      хвост потока ошибок:\n');
        for (const line of tail) process.stdout.write(`        ${line}\n`);
      } else {
        process.stdout.write('      поток ошибок пуст — процесс умер молча\n');
      }
      continue;
    }

    if (r.outcome === 'отказы') {
      process.stdout.write(
        `  ${r.suite.name}: отказы — ${r.failed} из ${r.count} (код выхода ${r.code})\n`,
      );
      continue;
    }

    process.stdout.write(`  ${r.suite.name}: пройден — ${r.count}\n`);
  }

  process.stdout.write('=======================================================\n');
  process.exit(exitCodeFor(results));
}

if (require.main === module) {
  main();
}

module.exports = { classify, runSuite, exitCodeFor, filesIn, SUITES, REPORTER };
