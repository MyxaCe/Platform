'use strict';
// ============================================================================
// sso.js — вход по handoff-токену платформы (ADR-023, фаза Т2). Контракт — sso-pack v1.
//
// Терминал живёт в iframe кабинета. Кабинет (родитель) присылает короткий handoff-JWT
// через postMessage; мы меняем его на сессию терминала (POST /v1/session) и держим
// bearer-токен В ПАМЯТИ (не в cookie: в iframe это third-party cookie — их режут браузеры;
// не в localStorage: перезагрузка → token.refresh.request вернёт токен за миллисекунды).
//
// Протокол postMessage (с проверкой origin с обеих сторон):
//   родитель → iframe : { type:'sso.token', token:'<jwt>' }   (onLoad и на refresh)
//   iframe → родитель : { type:'token.refresh.request' }        (за <2 мин до истечения)
//   родитель → iframe : { type:'sso.logout' }                   (сессия платформы погашена)
// ============================================================================
(function () {
  let sessionToken = null;
  let expiresAt = 0;
  let refreshTimer = null;
  const listeners = [];

  function token() { return sessionToken; }
  function onChange(fn) { listeners.push(fn); }
  function emit() { listeners.forEach((f) => { try { f(sessionToken); } catch { /* */ } }); }

  function origins(cfg) { return Array.isArray(cfg.cabinetOrigins) ? cfg.cabinetOrigins : []; }
  function postToParent(msg, cfg) {
    if (window.parent && window.parent !== window) {
      origins(cfg).forEach((o) => { try { window.parent.postMessage(msg, o); } catch { /* */ } });
    }
  }

  function scheduleRefresh(cfg) {
    if (refreshTimer) clearTimeout(refreshTimer);
    const lead = 2 * 60 * 1000; // просим новый токен за 2 минуты до истечения
    const delay = Math.max(1000, (expiresAt - Date.now()) - lead);
    refreshTimer = setTimeout(() => postToParent({ type: 'token.refresh.request' }, cfg), delay);
  }

  async function mint(jwt, cfg) {
    const site = cfg.site || new URLSearchParams(location.search).get('site') || '';
    try {
      const r = await fetch('/v1/session', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ token: jwt, site }),
      });
      if (!r.ok) { sessionToken = null; emit(); return; }
      const d = await r.json();
      sessionToken = d.token;
      expiresAt = Date.now() + (d.expires_in || 0) * 1000;
      scheduleRefresh(cfg);
      emit();
    } catch {
      sessionToken = null; emit();
    }
  }

  async function logout() {
    const t = sessionToken;
    sessionToken = null;
    if (refreshTimer) clearTimeout(refreshTimer);
    emit();
    if (t) { try { await fetch('/v1/session/logout', { method: 'POST', headers: { Authorization: 'Bearer ' + t } }); } catch { /* */ } }
  }

  function init(overrides) {
    const cfg = Object.assign({ site: '', cabinetOrigins: [] }, window.TERMINAL_CONFIG || {}, overrides || {});
    const allowed = origins(cfg);
    window.addEventListener('message', (e) => {
      // Принимаем сообщения только от разрешённых origin'ов кабинета.
      if (allowed.length && !allowed.includes(e.origin)) return;
      const m = e.data;
      if (!m || typeof m !== 'object') return;
      if (m.type === 'sso.token' && typeof m.token === 'string') mint(m.token, cfg);
      else if (m.type === 'sso.logout') logout();
    });
    return { token, onChange, logout };
  }

  window.Sso = { init, token, onChange, logout };
})();
