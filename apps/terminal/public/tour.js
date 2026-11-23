// @ts-check
/**
 * Экскурсия по терминалу (docs/site/интерактивное-обучение.md).
 *
 * Тексты не живут в терминале: их присылает кабинет (CMS → кабинет → postMessage),
 * поэтому без кабинета и без его сообщения терминал выглядит как прежде.
 * Протокол (только с разрешённых источников TERMINAL_CONFIG.cabinetOrigins):
 *   терминал → кабинет  { type: 'onboarding.tour.ready' }
 *   кабинет → терминал  { type: 'onboarding.tour.config', version, steps:[{anchor,title,body}], labels, autostart }
 *   терминал → кабинет  { type: 'onboarding.tour.result', version, status: 'done'|'skipped' }
 * Журнал счёта и шлюз не участвуют.
 *
 * Без :has()/popover/svh — Chrome 104 (ТД-001). Тексты — только textContent.
 */
(function () {
  'use strict';

  /** Имя элемента из CMS → селекторы (подсветка охватывает все найденные) и вкладка телефона. */
  const ANCHORS = {
    'terminal.pairs': { sel: ['#pairs'], phone: 'watch' },
    'terminal.categories': { sel: ['.watch__cats'], phone: 'watch' },
    'terminal.chart': { sel: ['#chart'], phone: 'chart' },
    'terminal.timeframes': { sel: ['#tfs'], phone: 'chart' },
    'terminal.deal': { sel: ['#deal'], phone: 'order' },
    'terminal.volume': { sel: ['.fld--size'], phone: 'order' },
    'terminal.sltp': { sel: ['.fld--sl', '.fld--tp'], phone: 'order' },
    'terminal.buySell': { sel: ['.deal__btns'], phone: 'order' },
    'terminal.positions': { sel: ['#bottom'], phone: 'positions' },
    'terminal.close': { sel: ['.closebtn'], phone: 'positions' },
    'terminal.help': { sel: ['#tourHelp'], phone: null },
  };

  const MAX_STEPS = 50;
  const MAX_TEXT = 1000;
  const PAD = 6;
  const GAP = 12;
  const CARD_W = 340;
  const PHONE_MAX = 860;

  /** @param {unknown} v @param {number} [max] */
  function str(v, max) { return typeof v === 'string' ? v.slice(0, max || MAX_TEXT) : ''; }

  /**
   * Проверка сообщения кабинета: только известные элементы, только строки.
   * @param {any} m
   */
  function parseConfig(m) {
    if (!m || m.type !== 'onboarding.tour.config' || !Array.isArray(m.steps)) return null;
    const version = Number.isInteger(m.version) && m.version >= 1 ? m.version : null;
    if (version === null) return null;
    const steps = m.steps.slice(0, MAX_STEPS)
      .filter((s) => s && typeof s.anchor === 'string' && Object.prototype.hasOwnProperty.call(ANCHORS, s.anchor))
      .map((s) => ({ anchor: s.anchor, title: str(s.title), body: str(s.body) }))
      .filter((s) => s.title.trim() !== '' || s.body.trim() !== '');
    if (steps.length === 0) return null;
    const l = m.labels && typeof m.labels === 'object' ? m.labels : {};
    const labels = {
      back: str(l.back, 40), next: str(l.next, 40), skip: str(l.skip, 40), done: str(l.done, 40),
      help: str(l.help, 80), stepOf: str(l.stepOf, 80),
    };
    return { version, steps, labels, autostart: m.autostart === true };
  }

  /** @param {Element} el */
  function visible(el) {
    if (!(el instanceof HTMLElement)) return false;
    if (el.closest('[hidden]')) return false;
    if (el.getClientRects().length === 0) return false;
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0;
  }

  /** @param {string} anchor @returns {HTMLElement[]} */
  function targets(anchor) {
    const def = ANCHORS[/** @type {keyof typeof ANCHORS} */ (anchor)];
    if (!def) return [];
    /** @type {HTMLElement[]} */
    const out = [];
    for (const sel of def.sel) {
      const el = Array.from(document.querySelectorAll(sel)).find(visible);
      if (el) out.push(/** @type {HTMLElement} */ (el));
    }
    return out;
  }

  /** @param {HTMLElement[]} els */
  function union(els) {
    let top = Infinity, left = Infinity, right = -Infinity, bottom = -Infinity;
    for (const el of els) {
      const r = el.getBoundingClientRect();
      top = Math.min(top, r.top); left = Math.min(left, r.left);
      right = Math.max(right, r.right); bottom = Math.max(bottom, r.bottom);
    }
    return { top, left, width: right - left, height: bottom - top };
  }

  /**
   * @param {{top:number,left:number,width:number,height:number}} box
   * @param {{width:number,height:number}} vp
   * @param {number} cardH
   */
  function placeCard(box, vp, cardH) {
    if (vp.width <= 640) return { mode: 'sheet' };
    const w = Math.min(CARD_W, vp.width - 2 * GAP);
    const left = Math.min(Math.max(GAP, box.left), vp.width - w - GAP);
    const below = box.top + box.height + PAD + GAP;
    if (below + cardH <= vp.height - GAP) return { mode: 'float', top: below, left };
    const above = box.top - PAD - GAP - cardH;
    if (above >= GAP) return { mode: 'float', top: above, left };
    return { mode: 'float', top: Math.max(GAP, vp.height - cardH - GAP), left };
  }

  const isPhone = () => window.innerWidth <= PHONE_MAX;

  /** Телефон: показать нужную вкладку (теми же кнопками, что нажимает клиент). @param {string|null} tab */
  function showPhoneTab(tab) {
    if (!isPhone() || !tab) return;
    if (tab === 'watch') { document.body.classList.add('m-watch'); return; }
    document.body.classList.remove('m-watch');
    const btn = /** @type {HTMLElement|null} */ (document.querySelector(`#mtabs [data-m="${tab}"]`));
    if (btn) btn.click();
  }

  const CSS = `
.tour-help { width: 28px; height: 28px; border-radius: 50%; border: 1px solid var(--border); background: transparent; color: var(--muted); font-weight: 700; font-size: 14px; line-height: 1; cursor: pointer; }
.tour-help:hover, .tour-help:focus-visible { color: var(--brand); border-color: var(--brand); }
.tour { position: fixed; top: 0; right: 0; bottom: 0; left: 0; z-index: 1000; }
.tour__catch { position: absolute; top: 0; right: 0; bottom: 0; left: 0; }
.tour__dim { position: absolute; top: 0; right: 0; bottom: 0; left: 0; background: rgba(0,0,0,.6); }
.tour__hl { position: fixed; border-radius: 10px; box-shadow: 0 0 0 2px var(--brand), 0 0 0 9999px rgba(0,0,0,.6); pointer-events: none; transition: top .2s, left .2s, width .2s, height .2s; }
.tour__card { position: fixed; width: ${CARD_W}px; max-width: calc(100vw - 24px); background: var(--panel); color: var(--text); border: 1px solid var(--border); border-radius: 14px; padding: 14px 16px; box-shadow: 0 10px 30px rgba(0,0,0,.4); outline: none; }
.tour__card--sheet { left: 0; right: 0; bottom: 0; width: auto; max-width: none; border-radius: 14px 14px 0 0; padding-bottom: calc(14px + env(safe-area-inset-bottom)); }
.tour__count { font-size: 11px; color: var(--muted); }
.tour__title { margin: 4px 0 0; font-size: 15px; font-weight: 600; }
.tour__body { margin: 6px 0 0; font-size: 13px; line-height: 1.5; color: var(--muted); white-space: pre-line; }
.tour__btns { display: flex; align-items: center; gap: 8px; margin-top: 14px; }
.tour__btns button { min-height: 36px; border-radius: 8px; padding: 0 12px; font-size: 13px; cursor: pointer; border: 1px solid var(--border); background: transparent; color: var(--text); }
.tour__btns .tour__skip { margin-right: auto; border: 0; color: var(--muted); padding: 0 4px; }
.tour__btns .tour__next { background: var(--brand); border-color: var(--brand); color: var(--on-brand, #111); font-weight: 600; }
.tour__btns button:disabled { opacity: .4; cursor: default; }
@media (prefers-reduced-motion: reduce) { .tour__hl { transition: none; } }
`;

  function injectCss() {
    if (document.getElementById('tourCss')) return;
    const s = document.createElement('style');
    s.id = 'tourCss';
    s.textContent = CSS;
    document.head.appendChild(s);
  }

  /**
   * Показ экскурсии. Возвращает функцию закрытия.
   * @param {{version:number, steps:{anchor:string,title:string,body:string}[], labels:Record<string,string>}} cfg
   * @param {(status:'done'|'skipped', shown:boolean) => void} onClose
   */
  function run(cfg, onClose) {
    injectCss();
    const prevTab = document.body.dataset.mtab || 'chart';
    const prevWatch = document.body.classList.contains('m-watch');
    const restoreFocus = document.activeElement;
    let shown = false;
    let index = -1;
    let closed = false;

    const root = document.createElement('div');
    root.className = 'tour';
    const catcher = document.createElement('div');
    catcher.className = 'tour__catch';
    catcher.addEventListener('click', (e) => e.stopPropagation());
    const hl = document.createElement('div');
    hl.className = 'tour__hl';
    hl.setAttribute('aria-hidden', 'true');
    const card = document.createElement('div');
    card.className = 'tour__card';
    card.setAttribute('role', 'dialog');
    card.setAttribute('aria-modal', 'true');
    card.tabIndex = -1;
    const count = document.createElement('p'); count.className = 'tour__count';
    const title = document.createElement('h2'); title.className = 'tour__title'; title.id = 'tourTitle';
    const body = document.createElement('p'); body.className = 'tour__body'; body.id = 'tourBody';
    card.setAttribute('aria-labelledby', 'tourTitle');
    card.setAttribute('aria-describedby', 'tourBody');
    const btns = document.createElement('div'); btns.className = 'tour__btns';
    const bSkip = document.createElement('button'); bSkip.type = 'button'; bSkip.className = 'tour__skip'; bSkip.textContent = cfg.labels.skip;
    const bBack = document.createElement('button'); bBack.type = 'button'; bBack.textContent = cfg.labels.back;
    const bNext = document.createElement('button'); bNext.type = 'button'; bNext.className = 'tour__next';
    btns.append(bSkip, bBack, bNext);
    card.append(count, title, body, btns);
    root.append(catcher, hl, card);

    /** @param {number} i */
    function has(i) {
      const step = cfg.steps[i];
      const def = ANCHORS[/** @type {keyof typeof ANCHORS} */ (step.anchor)];
      if (isPhone()) {
        // На телефоне элемент может быть на другой вкладке — проверяем, есть ли он в разметке вообще.
        return def.sel.some((sel) => document.querySelector(sel) !== null);
      }
      return targets(step.anchor).length > 0;
    }

    /** @param {number} from @param {1|-1} dir */
    function nextIndex(from, dir) {
      for (let i = from; i >= 0 && i < cfg.steps.length; i += dir) if (has(i)) return i;
      return -1;
    }

    function measure() {
      if (index < 0) return;
      const els = targets(cfg.steps[index].anchor);
      if (els.length === 0) { hl.style.display = 'none'; return; }
      hl.style.display = '';
      const box = union(els);
      const vp = { width: window.innerWidth, height: window.innerHeight };
      const t = Math.max(0, box.top - PAD), l = Math.max(0, box.left - PAD);
      hl.style.top = `${t}px`; hl.style.left = `${l}px`;
      hl.style.width = `${Math.min(vp.width, box.left + box.width + PAD) - l}px`;
      hl.style.height = `${Math.min(vp.height, box.top + box.height + PAD) - t}px`;
      const place = placeCard(box, vp, card.offsetHeight || 180);
      card.classList.toggle('tour__card--sheet', place.mode === 'sheet');
      card.style.top = place.mode === 'float' ? `${place.top}px` : '';
      card.style.left = place.mode === 'float' ? `${place.left}px` : '';
    }

    /** @param {number} i */
    function show(i) {
      index = i;
      const step = cfg.steps[i];
      const def = ANCHORS[/** @type {keyof typeof ANCHORS} */ (step.anchor)];
      showPhoneTab(def.phone);
      count.textContent = cfg.labels.stepOf.replace('{current}', String(i + 1)).replace('{total}', String(cfg.steps.length));
      title.textContent = step.title;
      body.textContent = step.body;
      bBack.disabled = nextIndex(i - 1, -1) === -1;
      const last = nextIndex(i + 1, 1) === -1;
      bNext.textContent = last ? cfg.labels.done : cfg.labels.next;
      requestAnimationFrame(() => {
        const els = targets(step.anchor);
        if (els.length === 0) { go(1); return; }
        shown = true;
        const reduce = window.matchMedia && window.matchMedia('(prefers-reduced-motion: reduce)').matches;
        els[0].scrollIntoView({ block: 'center', inline: 'nearest', behavior: reduce ? 'auto' : 'smooth' });
        measure();
        card.focus();
      });
    }

    /** @param {1|-1} dir */
    function go(dir) {
      const n = nextIndex(index + dir, dir);
      if (n === -1) { if (dir === 1) close('done'); return; }
      show(n);
    }

    /** @param {'done'|'skipped'} status */
    function close(status) {
      if (closed) return;
      closed = true;
      window.removeEventListener('keydown', onKey, true);
      window.removeEventListener('resize', onMove);
      window.removeEventListener('scroll', onMove, true);
      root.remove();
      if (isPhone()) {
        document.body.classList.toggle('m-watch', prevWatch);
        const btn = /** @type {HTMLElement|null} */ (document.querySelector(`#mtabs [data-m="${prevTab}"]`));
        if (btn) btn.click();
      }
      if (restoreFocus instanceof HTMLElement) restoreFocus.focus();
      onClose(status, shown);
    }

    /** @param {KeyboardEvent} e */
    function onKey(e) {
      if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); close('skipped'); }
      else if (e.key === 'ArrowRight') { e.preventDefault(); e.stopPropagation(); go(1); }
      else if (e.key === 'ArrowLeft') { e.preventDefault(); e.stopPropagation(); go(-1); }
    }
    let frame = 0;
    function onMove() { cancelAnimationFrame(frame); frame = requestAnimationFrame(measure); }

    bSkip.addEventListener('click', () => close('skipped'));
    bBack.addEventListener('click', () => go(-1));
    bNext.addEventListener('click', () => go(1));
    window.addEventListener('keydown', onKey, true);
    window.addEventListener('resize', onMove);
    window.addEventListener('scroll', onMove, true);

    const first = nextIndex(0, 1);
    if (first === -1) { closed = true; onClose('done', false); return () => {}; }
    document.body.appendChild(root);
    show(first);
    return () => close('skipped');
  }

  /**
   * Подключение к кабинету. Без разрешённых источников (терминал открыт сам по себе) — ничего.
   * @param {{ cabinetOrigins?: string[] }} [cfgIn]
   */
  function init(cfgIn) {
    const allowed = Array.isArray(cfgIn && cfgIn.cabinetOrigins) ? /** @type {string[]} */ (cfgIn && cfgIn.cabinetOrigins).filter(Boolean) : [];
    if (allowed.length === 0 || window.parent === window) return;
    /** @type {ReturnType<typeof parseConfig>} */
    let tour = null;
    /** @type {string|null} */
    let from = null;
    let running = false;

    function start() {
      if (!tour || running) return;
      running = true;
      const t = tour;
      run(t, (status, shown) => {
        running = false;
        if (shown && from) window.parent.postMessage({ type: 'onboarding.tour.result', version: t.version, status }, from);
      });
    }

    function mountButton() {
      if (!tour) return;
      let btn = /** @type {HTMLButtonElement|null} */ (document.getElementById('tourHelp'));
      if (!btn) {
        injectCss();
        btn = document.createElement('button');
        btn.id = 'tourHelp';
        btn.type = 'button';
        btn.className = 'tour-help';
        btn.textContent = '?';
        btn.addEventListener('click', start);
        const right = document.querySelector('.top__right');
        const theme = document.getElementById('themeToggle');
        if (right) right.insertBefore(btn, theme && theme.parentElement === right ? theme : right.firstChild);
      }
      btn.setAttribute('aria-label', tour.labels.help);
      btn.title = tour.labels.help;
    }

    window.addEventListener('message', (e) => {
      if (!allowed.includes(e.origin)) return;
      const parsed = parseConfig(e.data);
      if (!parsed) return;
      tour = parsed;
      from = e.origin;
      mountButton();
      if (parsed.autostart) setTimeout(start, 800);
    });
    for (const origin of allowed) {
      try { window.parent.postMessage({ type: 'onboarding.tour.ready' }, origin); } catch { /* чужой источник — сообщение не уйдёт */ }
    }
  }

  window.Tour = { init, run, parseConfig, placeCard, ANCHORS };
})();
