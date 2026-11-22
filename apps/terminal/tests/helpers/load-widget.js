'use strict';
const fs = require('node:fs');
const path = require('node:path');
const { JSDOM } = require('jsdom');

const PUBLIC_DIR = path.join(__dirname, '..', '..', 'public');

/**
 * @param {string[]} files
 * @param {{ url?: string }} [opts]
 * @returns {{ window: Window, document: Document, root: HTMLElement, cleanup: () => void }}
 */
function loadWidget(files, opts) {
  const dom = new JSDOM('<!doctype html><html><head></head><body><div id="root"></div></body></html>', {
    url: (opts && opts.url) || 'http://localhost:8888/',
    pretendToBeVisual: true,
    runScripts: 'outside-only',
  });
  for (const file of files) {
    const code = fs.readFileSync(path.join(PUBLIC_DIR, file), 'utf8');
    dom.window.eval(code);
  }
  return {
    window: dom.window,
    document: dom.window.document,
    root: dom.window.document.getElementById('root'),
    cleanup: () => dom.window.close(),
  };
}

function rowIds(root) {
  return [...root.querySelectorAll('.irow')].map((el) => Number(el.dataset.id));
}

function click(window, el) {
  el.dispatchEvent(new window.MouseEvent('click', { bubbles: true }));
}

module.exports = { loadWidget, rowIds, click };
