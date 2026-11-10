'use strict';
// ============================================================================
// sso.js — вход по handoff-токену платформы (ADR-023, фаза Т2). Контракт — sso-pack v1.
//
// Два способа открыть терминал, и оба живут одновременно:
//
// 1) ВО ФРЕЙМЕ КАБИНЕТА (как было). Кабинет (родитель) присылает короткий
//    handoff-JWT через postMessage; мы меняем его на сессию терминала
//    (POST /v1/session) и держим bearer-токен В ПАМЯТИ (не в cookie: в iframe
//    это third-party cookie — их режут браузеры; не в localStorage: перезагрузка
//    → token.refresh.request вернёт токен за миллисекунды).
//
//    Протокол postMessage (с проверкой origin с обеих сторон):
//      родитель → iframe : { type:'sso.token', token:'<jwt>' }   (onLoad и на refresh)
//      iframe → родитель : { type:'token.refresh.request' }        (за <2 мин до истечения)
//      родитель → iframe : { type:'sso.logout' }                   (сессия платформы погашена)
//
// 2) ВЕРХНИМ УРОВНЕМ (10.11, решение штаба) — самостоятельная страница:
//      https://terminal…/?site=<slug>#handoff=<jwt>&account=demo|real[&lang=ru|en]
//    • токен приходит во ФРАГМЕНТЕ: фрагмент не уходит на сервер, не пишется в
//      логи прокси и не попадает в Referer;
//    • фрагмент стирается СРАЗУ (`history.replaceState`), до обмена: токен не
//      остаётся в адресной строке, в истории и в закладке;
//    • продление без родителя — переход верхним уровнем на
//        <кабинет>/api/sso/launch?account=<demo|real>&return=<адрес терминала>
//      кабинет проверяет свою сессию и `return` по СВОЕМУ белому списку и
//      возвращает сюда с новым `#handoff=…`;
//    • «В кабинет» ведёт только на origin из `cabinetOrigins` конфига: адрес из
//      ссылки или Referer принимается, лишь если он в этом списке. Иначе это
//      был бы открытый редирект с нашего домена.
//
// Чистые функции (`parseFragment`, `pickCabinet`, `launchUrl`) экспортированы
// ради тестов: в них вся граница безопасности, и проверять её через браузер
// значило бы проверять заодно браузер.
// ============================================================================
(function () {
  const ACCOUNT_KEY = 'terminal:sso.account';
  const CABINET_KEY = 'terminal:sso.cabinet';
  const RELAUNCH_KEY = 'terminal:sso.relaunchAt';
  /** Не чаще одного перехода на кабинет в минуту: защита от петли «кабинет
   *  вернул негодный токен → снова в кабинет». */
  const RELAUNCH_MIN_MS = 60 * 1000;

  let sessionToken = null;
  let expiresAt = 0;
  let refreshTimer = null;
  let warnTimer = null;
  /** Чем закончился последний вход: 'none' | 'ok' | 'failed' | 'expired'. */
  let state = 'none';
  let account = null;
  let cabinet = null;
  let mode = 'iframe';
  /** @type {any} */
  let cfgRef = {};
  const listeners = [];

  function token() { return sessionToken; }
  function onChange(fn) { listeners.push(fn); }
  function emit() { listeners.forEach((f) => { try { f(sessionToken); } catch { /* */ } }); }

  function origins(cfg) { return Array.isArray(cfg.cabinetOrigins) ? cfg.cabinetOrigins.filter(Boolean) : []; }

  // ---- Чистые функции границы --------------------------------------------

  /**
   * Разобрать фрагмент входной ссылки. Чужие ключи игнорируются.
   * @param {string} hash `#handoff=…&account=…`
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

  /** Origin строки-адреса или `null`, если это не http(s)-адрес. */
  function originOf(url) {
    try {
      const u = new URL(String(url));
      return u.protocol === 'https:' || u.protocol === 'http:' ? u.origin : null;
    } catch { return null; }
  }

  /**
   * Какой кабинет считать «своим». Кандидаты по порядку доверия (ссылка,
   * Referer, сохранённый) принимаются ТОЛЬКО при точном совпадении origin с
   * белым списком; иначе — первый из списка. Пустой список — кабинета нет.
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
   * Адрес продления сессии верхним уровнем (маршрут кабинета).
   * @param {string} cabinetOrigin @param {string|null} acc @param {string} returnUrl
   */
  function launchUrl(cabinetOrigin, acc, returnUrl) {
    const q = new URLSearchParams();
    if (acc) q.set('account', acc);
    q.set('return', returnUrl);
    return `${cabinetOrigin}/api/sso/launch?${q}`;
  }

  // ---- Состояние вне памяти: только то, что не секрет ----------------------
  // Токены здесь не хранятся НИКОГДА. Род счёта и origin кабинета нужны, чтобы
  // после перезагрузки страницы знать, куда идти за новым токеном.
  const ss = {
    get(k) { try { return sessionStorage.getItem(k); } catch { return null; } },
    set(k, v) { try { if (v == null) sessionStorage.removeItem(k); else sessionStorage.setItem(k, v); } catch { /* */ } },
  };

  function isTopLevel() {
    try { return window.top === window; } catch { return false; }
  }

  /** Адрес, на который кабинет вернёт человека: эта страница без фрагмента. */
  function returnUrl() { return location.origin + location.pathname + location.search; }

  function cabinetUrl() { return cabinet ? `${cabinet}/` : null; }

  /** Уйти в кабинет за новым токеном. `force` — по кнопке человека, без троттлинга. */
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
      origins(cfg).forEach((o) => { try { window.parent.postMessage(msg, o); } catch { /* */ } });
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
    const lead = 2 * 60 * 1000; // просим новый токен за 2 минуты до истечения
    // Нижняя граница осталась, но теперь она страхует короткий срок, а не
    // подменяет отсутствующий: пустой `expires_in` до сюда больше не доходит.
    const delay = Math.max(1000, (expiresAt - Date.now()) - lead);
    if (mode === 'iframe') {
      refreshTimer = setTimeout(() => postToParent({ type: 'token.refresh.request' }, cfg), delay);
      return;
    }
    // Верхний уровень: родителя нет. За 2 минуты — предупреждение с кнопкой
    // «Продлить» (хост его покажет), за 20 секунд — переход сам: сессия всё
    // равно кончится, а вернуться с новой быстрее, чем объяснять отказ.
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

      /**
       * Срок жизни сессии обязателен, и его отсутствие — отказ (TRM-24).
       *
       * Было `(d.expires_in || 0)`: пустой срок превращался в «истекла
       * только что», `scheduleRefresh` считал задержку по нижней границе и
       * терминал просил у кабинета новый токен **раз в секунду**, бесконечно.
       *
       * Это умолчание хуже обычного вранья: оно не показывает неправду, а
       * **производит работу** — отказ, замаскированный под нагрузку. Со
       * стороны кабинета это выглядит как исправно работающий клиент.
       *
       * Почему отказываем, а не берём сессию без обновления: контракт
       * `sso-pack` требует тихого ре-handoff до истечения, а не зная срока,
       * исполнить его нечем. Сессия, которая молча умрёт посреди работы, —
       * тот же отказ, только отложенный и на стороне человека. Отказ громкий
       * и сразу чинится, отложенный — объясняется.
       */
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
      // След в состоянии был и раньше (сессия становится пустой, интерфейс
      // показывает «нет сессии»), а вот ПРИЧИНА терялась: сеть, не-2xx и
      // непригодный ответ схлопывались в один молчаливый отказ. Наружу
      // по-прежнему виден один исход — подсказывать подбирающему нечего, —
      // но изнутри отличить обрыв от отказа шлюза теперь можно.
      console.error('[sso] обмен handoff-токена не состоялся', e);
      sessionToken = null; setState('failed');
    }
  }

  /**
   * Выход. Во фрейме — по команде кабинета; верхним уровнем — по кнопке, и
   * тогда после погашения сессии уходим на страницу выхода терминала.
   * @param {{ redirect?: boolean }} [opts]
   */
  async function logout(opts) {
    const t = sessionToken;
    sessionToken = null;
    clearTimers();
    setState('none');
    if (t) { try { await fetch('/v1/session/logout', { method: 'POST', headers: { Authorization: 'Bearer ' + t } }); } catch { /* */ } }
    if (opts && opts.redirect) {
      ss.set(RELAUNCH_KEY, null);
      // Род счёта и кабинет оставляем: страница выхода ведёт «В кабинет» и
      // «Открыть снова» по ним. Токенов в sessionStorage нет и не было.
      location.assign('/logout' + location.search);
    }
  }

  /** Сколько миллисекунд осталось сессии; 0 — сессии нет. */
  function remainingMs() { return sessionToken ? Math.max(0, expiresAt - Date.now()) : 0; }

  function init(overrides) {
    const cfg = Object.assign({ site: '', cabinetOrigins: [] }, window.TERMINAL_CONFIG || {}, overrides || {});
    cfgRef = cfg;
    const allowed = origins(cfg);
    mode = isTopLevel() ? 'top' : 'iframe';

    // ---- Фрагмент входной ссылки: прочитать и СРАЗУ стереть ----------------
    const frag = parseFragment(location.hash);
    if (frag.any) {
      try { history.replaceState(history.state, '', location.pathname + location.search); } catch { /* */ }
    }
    if (frag.account) ss.set(ACCOUNT_KEY, frag.account);
    account = frag.account || ss.get(ACCOUNT_KEY);

    let ref = null;
    try { ref = document.referrer || null; } catch { /* */ }
    cabinet = pickCabinet([frag.cabinet, ref, ss.get(CABINET_KEY)], allowed);
    if (cabinet) ss.set(CABINET_KEY, cabinet);

    window.addEventListener('message', (e) => {
      // Принимаем сообщения только от разрешённых origin'ов кабинета.
      if (allowed.length && !allowed.includes(e.origin)) return;
      const m = e.data;
      if (!m || typeof m !== 'object') return;
      if (m.type === 'sso.token' && typeof m.token === 'string') mint(m.token, cfg);
      else if (m.type === 'sso.logout') logout();
    });

    if (frag.handoff) {
      mint(frag.handoff, cfg);
    } else if (mode === 'top' && allowed.length && account && cabinet) {
      // Перезагрузка страницы верхнего уровня: токена в памяти больше нет, но
      // известно, откуда он пришёл. Идём за новым сами (не чаще раза в минуту).
      relaunch(false);
    }
    return { token, onChange, logout, relaunch, mode: () => mode, state: () => state, cabinetUrl, remainingMs };
  }

  window.Sso = {
    init, token, onChange, logout, relaunch, cabinetUrl, remainingMs,
    mode: () => mode,
    state: () => state,
    account: () => account,
    /** Встроен ли терминал в платформу (задан кабинет) — торговля требует сессию. */
    embedded: () => origins(cfgRef).length > 0,
    parseFragment, pickCabinet, launchUrl,
  };
})();
