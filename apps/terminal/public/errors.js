(function () {
  var shown = [];
  var started = Date.now();
  var box = null;

  function render() {
    if (!document.body) return;
    if (!box) {
      box = document.createElement('div');
      box.id = 'fatalErrors';
      box.setAttribute('role', 'alert');
      box.style.cssText = 'position:fixed;left:8px;right:8px;bottom:8px;z-index:2147483647;max-height:45%;overflow:auto;' +
        'background:#3a0d12;color:#ffd7db;border:1px solid #ff5a6a;border-radius:8px;padding:10px 12px;' +
        'font:12px/1.4 -apple-system,Segoe UI,Roboto,sans-serif;white-space:pre-wrap;word-break:break-word;';
      document.body.appendChild(box);
    }
    box.textContent = 'Терминал не смог загрузиться полностью. Пришлите, пожалуйста, скриншот этого сообщения в поддержку.\n\n' +
      shown.join('\n\n') + '\n\n' + navigator.userAgent;
  }

  function add(text) {
    if (Date.now() - started > 30000) return;
    if (shown.length >= 5 || shown.indexOf(text) !== -1) return;
    shown.push(text);
    if (document.body) render();
    else document.addEventListener('DOMContentLoaded', render);
  }

  function where(file, line, col) {
    if (!file) return '';
    var name = String(file).split('?')[0].split('/').pop();
    return ' (' + name + (line ? ':' + line : '') + (col ? ':' + col : '') + ')';
  }

  window.addEventListener('error', function (e) {
    var target = /** @type {any} */ (e && e.target);
    if (target && target !== window && target.tagName) {
      if (target.tagName === 'SCRIPT') add('Не загрузился файл: ' + where(target.src || target.href).slice(2, -1));
      return;
    }
    var msg = (e && e.message) || (e && e.error && String(e.error)) || 'неизвестная ошибка';
    add(msg + where(e && e.filename, e && e.lineno, e && e.colno));
  }, true);

  /** @type {any} */ (window).TerminalErrors = { shown: function () { return shown.slice(); } };
})();
