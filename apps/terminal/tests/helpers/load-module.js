'use strict';
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const PUBLIC_DIR = path.join(__dirname, '..', '..', 'public');

/**
 * @param {string} file
 * @param {string} exportName
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
