'use strict';
// ============================================================================
// Загрузка браузерного модуля терминала в Node без сборки и без зависимостей.
//
// Модули по ADR-015 — IIFE, которые кладут себя в `window.<Имя>`. Здесь мы даём
// им минимальный `window` и забираем экспорт. Никакого jsdom: чистым функциям
// (индикаторы) DOM не нужен, а тянуть ради них полновесный браузер значит
// сделать тесты медленными и хрупкими — тогда их перестанут гонять.
// ============================================================================
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const PUBLIC_DIR = path.join(__dirname, '..', '..', 'public');

/**
 * Выполнить модуль из public/ в песочнице и вернуть то, что он положил в window.
 * @param {string} file имя файла, например 'indicators.js'
 * @param {string} exportName имя в window, например 'Indicators'
 */
function loadModule(file, exportName) {
  const code = fs.readFileSync(path.join(PUBLIC_DIR, file), 'utf8');
  const sandbox = { window: {}, document: undefined, console };
  sandbox.globalThis = sandbox;
  vm.createContext(sandbox);
  vm.runInContext(code, sandbox, { filename: file });
  const exported = sandbox.window[exportName];
  if (!exported) {
    throw new Error(`${file} не положил ${exportName} в window`);
  }
  return exported;
}

module.exports = { loadModule };
