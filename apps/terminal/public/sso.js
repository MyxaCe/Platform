'use strict';
(function () {
  const ACCOUNT_KEY = 'terminal:sso.account';
  const CABINET_KEY = 'terminal:sso.cabinet';
  const RELAUNCH_KEY = 'terminal:sso.relaunchAt';
  const RELAUNCH_MIN_MS = 60 * 1000;

  let sessionToken = null;
  let expiresAt = 0;
  let refreshTimer = null;
  let warnTimer = null;
  let state = 'none';
  let account = null;
  let cabinet = null;
  let mode = 'iframe';
  /** @type {any} */
  let cfgRef = {};
  const listeners = [];

  function token() { return sessionToken; }
  function onChange(fn) { listeners.push(fn); }
  function emit() { listeners.forEach((f) => { try { f(sessionToken); } catch {  } }); }

  function origins(cfg) { return Array.isArray(cfg.cabinetOrigins) ? cfg.cabinetOrigins.filter(Boolean) : []; }

  /**
   * @param {string} hash
   * @returns {{ handoff: string|null, account: 'demo'|'real'|null, lang: string|null, cabinet: string|null, any: boolean }}
   */
  function parseFragment(hash) {
    const p = new URLSearchParams(String(hash || '').replace(/^#/, ''));
    const acc = p.get('account');
    const handoff = p.get('handoff');
    return {
      handoff: handoff && handoff.length > 0 ? handoff : null,
      account: acc === 'demo' || acc === 'real' ? acc : null,
      lang: p.get('lang'),
      cabinet: p.get('cabinet'),
      any: p.has('handoff') || p.has('account') || p.has('cabinet'),
    };
  }

  function originOf(url) {
    try {
      const u = new URL(String(url));
      return u.protocol === 'https:' || u.protocol === 'http:' ? u.origin : null;
    } catch { return null; }
  }

  /**
   * @param {Array<string|null|undefined>} candidates
   * @param {string[]} allowed
   * @returns {string|null}
   */
  function pickCabinet(candidates, allowed) {
    const list = (allowed || []).map(originOf).filter(Boolean);
    if (!list.length) return null;
    for (const c of candidates) {
      const o = c ? originOf(c) : null;
      if (o && list.includes(o)) return o;
    }
    return list[0];
  }

  /**
   * @param {string} cabinetOrigin
   * @param {string|null} acc
   * @param {string} returnUrl
   */
  function launchUrl(cabinetOrigin, acc, returnUrl) {
    const q = new URLSearchParams();
    if (acc) q.set('account', acc);
    q.set('return', returnUrl);
    return `${cabinetOrigin}/api/sso/launch?${q}`;
  }

  const ss = {
    get(k) { try { return sessionStorage.getItem(k); } catch { return null; } },
    set(k, v) { try { if (v == null) sessionStorage.removeItem(k); else sessionStorage.setItem(k, v); } catch {  } },
  };

  function isTopLevel() {
    try { return window.top === window; } catch { return false; }
  }

  function returnUrl() { return location.origin + location.pathname + location.search; }

  function cabinetUrl() { return cabinet ? `${cabinet}/` : null; }

  function relaunch(force) {
    if (!cabinet) return false;
    const last = Number(ss.get(RELAUNCH_KEY) || 0);
    if (!force && Date.now() - last < RELAUNCH_MIN_MS) return false;
    ss.set(RELAUNCH_KEY, String(Date.now()));
    location.assign(launchUrl(cabinet, account, returnUrl()));
    return true;
  }

  function postToParent(msg, cfg) {
    if (window.parent && window.parent !== window) {
      origins(cfg).forEach((o) => { try { window.parent.postMessage(msg, o); } catch {  } });
    }
  }

  function setState(s) { state = s; emit(); }

  function clearTimers() {
    if (refreshTimer) clearTimeout(refreshTimer);
    if (warnTimer) clearTimeout(warnTimer);
    refreshTimer = null; warnTimer = null;
  }

  function scheduleRefresh(cfg) {
    clearTimers();
    const lead = 2 * 60 * 1000;
    const delay = Math.max(1000, (expiresAt - Date.now()) - lead);
    if (mode === 'iframe') {
      refreshTimer = setTimeout(() => postToParent({ type: 'token.refresh.request' }, cfg), delay);
      return;
    }
    warnTimer = setTimeout(() => emit(), delay);
    refreshTimer = setTimeout(() => {
      if (!relaunch(false)) { sessionToken = null; setState('expired'); }
    }, Math.max(1000, expiresAt - Date.now() - 20 * 1000));
  }

  async function mint(jwt, cfg) {
    const site = cfg.site || new URLSearchParams(location.search).get('site') || '';
    try {
      const r = await fetch('/v1/session', {
        method: 'POST',
        headers: { 'Content-Type': 'application/json' },
        body: JSON.stringify({ token: jwt, site }),
      });
      if (!r.ok) {
        console.error(`[sso] шлюз отклонил handoff-токен: HTTP ${r.status}`);
        sessionToken = null; setState('failed'); return;
      }
      const d = await r.json();

      const ttl = Number(d.expires_in);
      if (!Number.isFinite(ttl) || ttl <= 0) {
        console.error(
          '[sso] шлюз не назвал срок жизни сессии (expires_in=' +
          JSON.stringify(d.expires_in) + '): обновление исполнить нечем, сессия не принята',
        );
        sessionToken = null; setState('failed'); return;
      }

      sessionToken = d.token;
      expiresAt = Date.now() + ttl * 1000;
      scheduleRefresh(cfg);
      setState('ok');
    } catch (e) {
      console.error('[sso] обмен handoff-токена не состоялся', e);
      sessionToken = null; setState('failed');
    }
  }

  /** @param {{ redirect?: boolean }} [opts] */
  async function logout(opts) {
    const t = sessionToken;
    sessionToken = null;
    clearTimers();
    setState('none');
    if (t) { try { await fetch('/v1/session/logout', { method: 'POST', headers: { Authorization: 'Bearer ' + t } }); } catch {  } }
    if (opts && opts.redirect) {
      ss.set(RELAUNCH_KEY, null);
      location.assign('/logout' + location.search);
    }
  }

  function remainingMs() { return sessionToken ? Math.max(0, expiresAt - Date.now()) : 0; }

  function init(overrides) {
    const cfg = Object.assign({ site: '', cabinetOrigins: [] }, window.TERMINAL_CONFIG || {}, overrides || {});
    cfgRef = cfg;
    const allowed = origins(cfg);
    mode = isTopLevel() ? 'top' : 'iframe';

    const frag = parseFragment(location.hash);
    if (frag.any) {
      try { history.replaceState(history.state, '', location.pathname + location.search); } catch {  }
    }
    if (frag.account) ss.set(ACCOUNT_KEY, frag.account);
    account = frag.account || ss.get(ACCOUNT_KEY);

    let ref = null;
    try { ref = document.referrer || null; } catch {  }
    cabinet = pickCabinet([frag.cabinet, ref, ss.get(CABINET_KEY)], allowed);
    if (cabinet) ss.set(CABINET_KEY, cabinet);

    window.addEventListener('message', (e) => {
      if (allowed.length && !allowed.includes(e.origin)) return;
      const m = e.data;
      if (!m || typeof m !== 'object') return;
      if (m.type === 'sso.token' && typeof m.token === 'string') mint(m.token, cfg);
      else if (m.type === 'sso.logout') logout();
    });

    if (frag.handoff) {
      mint(frag.handoff, cfg);
    } else if (mode === 'top' && allowed.length && account && cabinet) {
      relaunch(false);
    }
    return { token, onChange, logout, relaunch, mode: () => mode, state: () => state, cabinetUrl, remainingMs };
  }

  window.Sso = {
    init, token, onChange, logout, relaunch, cabinetUrl, remainingMs,
    mode: () => mode,
    state: () => state,
    account: () => account,
    embedded: () => origins(cfgRef).length > 0,
    parseFragment, pickCabinet, launchUrl,
  };
})();
