'use strict';
(function () {
  const CSS = `
.lvl-back { position: fixed; inset: 0; z-index: 80; background: rgba(0,0,0,.45); display: flex; align-items: center; justify-content: center; }
.lvl-dlg { width: 400px; max-width: calc(100vw - 24px); background: var(--panel,#10141d); color: var(--text,#d7dce5); border: 1px solid var(--border,#1f2735);
  border-radius: 8px; box-shadow: 0 12px 40px rgba(0,0,0,.45); display: flex; flex-direction: column; font-size: 12.5px; }
.lvl-dlg__head { display: flex; align-items: center; gap: 8px; padding: 12px 14px; border-bottom: 1px solid var(--hair, var(--border,#1f2735)); }
.lvl-dlg__title { font-weight: 600; font-size: 13.5px; flex: 1; }
.lvl-dlg__sd { font-weight: 600; } .lvl-dlg__sd.buy { color: var(--up,#26a69a); } .lvl-dlg__sd.sell { color: var(--down,#ef5350); }
.lvl-dlg__x { background: transparent; border: 0; color: var(--muted,#6b7688); font-size: 16px; cursor: pointer; width: 28px; height: 28px; border-radius: 4px; }
.lvl-dlg__x:hover { color: var(--text,#d7dce5); background: var(--field-hover, var(--panel3,#1a2130)); }
.lvl-dlg__body { padding: 12px 14px; display: flex; flex-direction: column; gap: 10px; }
.lvl-dlg__ref { color: var(--muted,#6b7688); font-size: 11.5px; font-variant-numeric: tabular-nums; }
.lvl-f { display: grid; grid-template-columns: 1fr auto auto; gap: 6px; align-items: end; }
.lvl-f label { grid-column: 1 / -1; color: var(--muted,#6b7688); font-size: 11.5px; }
.lvl-f input { height: 34px; min-width: 0; background: var(--field, var(--panel2,#151a25)); border: 1px solid transparent; border-radius: 4px; color: var(--text,#d7dce5);
  padding: 0 10px; font: inherit; font-size: 13px; font-variant-numeric: tabular-nums; }
.lvl-f input:focus { outline: none; border-color: var(--brand,#f0b90b); }
.lvl-f input.bad, .lvl-f input.bad:focus { border-color: var(--down,#ef5350); color: var(--down,#ef5350); }
.lvl-f button { height: 34px; padding: 0 10px; background: transparent; border: 1px solid var(--border,#1f2735); color: var(--muted,#6b7688); border-radius: 4px; cursor: pointer; font: inherit; font-size: 11.5px; white-space: nowrap; }
.lvl-f button:hover { color: var(--text,#d7dce5); background: var(--field-hover, var(--panel3,#1a2130)); }
.lvl-dlg__msg { min-height: 16px; font-size: 11.5px; color: var(--down,#ef5350); }
.lvl-dlg__msg.ok { color: var(--muted,#6b7688); }
.lvl-dlg__foot { display: flex; gap: 8px; padding: 0 14px 14px; }
.lvl-dlg__foot button { flex: 1; height: 36px; border-radius: 4px; cursor: pointer; font: inherit; font-weight: 600; font-size: 13px; }
.lvl-dlg__cancel { background: transparent; border: 1px solid var(--border,#1f2735); color: var(--text,#d7dce5); }
.lvl-dlg__save { background: var(--brand,#f0b90b); border: 1px solid var(--brand,#f0b90b); color: var(--on-accent,#08090c); }
.lvl-dlg__save:disabled { opacity: .45; cursor: default; }
@media (max-width: 860px) {
  .lvl-back { align-items: flex-end; }
  .lvl-dlg { width: 100%; max-width: 100%; border-radius: 12px 12px 0 0; border-bottom: 0; padding-bottom: env(safe-area-inset-bottom); font-size: 13.5px; }
  .lvl-dlg__head { padding: 14px 16px; } .lvl-dlg__title { font-size: 15px; }
  .lvl-dlg__body { padding: 14px 16px; gap: 14px; }
  .lvl-f input { height: 44px; font-size: 16px; }
  .lvl-f button { height: 44px; padding: 0 12px; font-size: 13px; }
  .lvl-dlg__foot { padding: 0 16px 16px; }
  .lvl-dlg__foot button { height: 48px; font-size: 15px; }
}
`;
  function injectCss() {
    if (typeof document === 'undefined' || document.getElementById('levels-css')) return;
    const s = document.createElement('style'); s.id = 'levels-css'; s.textContent = CSS;
    document.head.appendChild(s);
  }

  /**
   * @param {string} text
   * @param {number} pd
   * @returns {number|null}
   */
  function parse(text, pd) {
    const s = String(text == null ? '' : text).trim().replace(',', '.');
    if (s === '') return null;
    const v = Number(s);
    if (!Number.isFinite(v) || v <= 0) return NaN;
    return Math.round(v * 10 ** pd);
  }

  /**
   * @param {'buy'|'sell'} side
   * @param {'sl'|'tp'} k
   * @param {number} level
   * @param {number} ref
   */
  function reached(side, k, level, ref) {
    const buy = side === 'buy';
    if (k === 'sl') return buy ? ref <= level : ref >= level;
    return buy ? ref >= level : ref <= level;
  }

  /**
   * @param {{ side: 'buy'|'sell', ref: number|null, cur: {sl: number|null, tp: number|null}, next: {sl: number|null, tp: number|null}, bps: number }} a
   * @returns {{ sl: null | { code: string, limit?: number }, tp: null | { code: string, limit?: number } }}
   */
  function evaluate(a) {
    /** @type {{ sl: any, tp: any }} */
    const out = { sl: null, tp: null };
    for (const k of /** @type {Array<'sl'|'tp'>} */ (['sl', 'tp'])) {
      const v = a.next[k];
      if (Number.isNaN(v)) { out[k] = { code: 'invalid_level' }; continue; }
      if (v == null || v === a.cur[k] || a.ref == null) continue;
      if (reached(a.side, k, v, a.ref)) { out[k] = { code: k === 'sl' ? 'stop_loss_already_reached' : 'take_profit_already_reached' }; continue; }
      const bad = window.Stops.check(a.side, a.ref, k === 'sl' ? v : null, k === 'tp' ? v : null, a.bps);
      if (bad) out[k] = { code: bad.code, limit: bad.limit };
    }
    return out;
  }

  /**
   * @param {{sl: number|null, tp: number|null}} cur
   * @param {{sl: number|null, tp: number|null}} next
   * @returns {Record<string, number|null> | null}
   */
  function body(cur, next) {
    /** @type {Record<string, number|null>} */
    const b = {};
    if (next.sl !== cur.sl) b.sl = next.sl;
    if (next.tp !== cur.tp) b.tp = next.tp;
    return Object.keys(b).length ? b : null;
  }

  let current = null;

  function close() {
    if (!current) return;
    current.destroy();
    current = null;
  }

  /** @param {any} o */
  function open(o) {
    close();
    injectCss();
    const t = o.t || ((/** @type {string} */ _k, /** @type {string} */ en, /** @type {any} */ v) => {
      let s = en; if (v) for (const k of Object.keys(v)) s = s.split(`{${k}}`).join(String(v[k])); return s;
    });
    const row = o.row;
    const pd = row.price_decimals;
    const num = o.num || ((/** @type {number} */ v, /** @type {number} */ n) => v.toLocaleString('en-US', { minimumFractionDigits: n, maximumFractionDigits: n }));
    const fmt = (/** @type {number} */ raw) => num(raw / 10 ** pd, pd);
    const inputVal = (/** @type {number|null} */ raw) => (raw == null ? '' : (raw / 10 ** pd).toFixed(pd));
    const cur = { sl: row.sl == null ? null : row.sl, tp: row.tp == null ? null : row.tp };
    const lot = num(row.qty / 10 ** row.qty_decimals, row.qty_decimals);
    const isDeal = o.kind === 'deal';

    const back = document.createElement('div');
    back.className = 'lvl-back';
    back.innerHTML =
      `<div class="lvl-dlg" role="dialog" aria-modal="true" aria-labelledby="lvlTitle">` +
      `<div class="lvl-dlg__head"><span class="lvl-dlg__title" id="lvlTitle">${t('lv.title', 'Edit SL/TP')} · ${row.symbol} <span class="lvl-dlg__sd ${row.side}">${row.side.toUpperCase()}</span> ${lot}</span>` +
      `<button type="button" class="lvl-dlg__x" data-act="close" aria-label="${t('lv.cancel', 'Cancel')}">✕</button></div>` +
      `<div class="lvl-dlg__body"><div class="lvl-dlg__ref" id="lvlRef"></div>` +
      ['sl', 'tp'].map((k) => `<div class="lvl-f"><label for="lvl${k === 'sl' ? 'Sl' : 'Tp'}">${k === 'sl' ? t('f.sl', 'Stop Loss') : t('f.tp', 'Take Profit')}</label>` +
        `<input id="lvl${k === 'sl' ? 'Sl' : 'Tp'}" data-k="${k}" type="text" inputmode="decimal" autocomplete="off" placeholder="—" value="${inputVal(cur[k])}" />` +
        `<button type="button" data-act="min" data-k="${k}" title="${t('lv.min.tip', 'Put the level at the minimum allowed distance')}">${t('lv.min', 'Min')}</button>` +
        `<button type="button" data-act="clear" data-k="${k}" title="${t('lv.clear.tip', 'Remove this level')}">${t('lv.clear', 'Remove')}</button></div>`).join('') +
      `<div class="lvl-dlg__msg" id="lvlMsg" aria-live="polite"></div></div>` +
      `<div class="lvl-dlg__foot"><button type="button" class="lvl-dlg__cancel" data-act="close">${t('lv.cancel', 'Cancel')}</button>` +
      `<button type="button" class="lvl-dlg__save" data-act="save">${t('lv.save', 'Save')}</button></div></div>`;
    document.body.appendChild(back);

    const q = (/** @type {string} */ s) => /** @type {HTMLElement} */ (back.querySelector(s));
    const input = (/** @type {'sl'|'tp'} */ k) => /** @type {HTMLInputElement} */ (q(`input[data-k="${k}"]`));
    const msg = q('#lvlMsg');
    const save = /** @type {HTMLButtonElement} */ (q('[data-act="save"]'));
    let busy = false;
    let serverMsg = '';

    const next = () => ({ sl: parse(input('sl').value, pd), tp: parse(input('tp').value, pd) });
    const textFor = (/** @type {'sl'|'tp'} */ k, /** @type {{code: string, limit?: number}} */ p) => {
      const name = k === 'sl' ? t('f.sl', 'Stop Loss') : t('f.tp', 'Take Profit');
      if (p.code === 'invalid_level') return t('lv.invalid', '{f}: enter a price above zero or leave it empty', { f: name });
      if (p.code.endsWith('_already_reached')) return t('lv.reached', '{f} is already on the other side of the price — it would trigger immediately', { f: name });
      return t(k === 'sl' ? 'f.sl.bad' : 'f.tp.bad', `${name} is closer than the minimum — nearest allowed {px}`, { px: fmt(/** @type {number} */ (p.limit)) });
    };

    function render() {
      const ref = o.ref();
      const bps = o.bps() || 0;
      const refEl = q('#lvlRef');
      if (ref == null) refEl.textContent = t('lv.noPrice', 'No price yet — the server will check the levels');
      else {
        const what = isDeal ? t(row.side === 'buy' ? 'lv.ref.bid' : 'lv.ref.ask', row.side === 'buy' ? 'Closing price (bid) {px}' : 'Closing price (ask) {px}', { px: fmt(ref) })
          : t('lv.ref.order', 'Order price {px}', { px: fmt(ref) });
        const min = bps > 0 ? ' · ' + t('lv.minDist', 'min. distance {p}% = {d}', { p: num(bps / 100, 2), d: fmt(window.Stops.minDistance(ref, bps)) }) : '';
        refEl.textContent = what + min;
      }
      const n = next();
      const ev = evaluate({ side: row.side, ref, cur, next: n, bps });
      input('sl').classList.toggle('bad', !!ev.sl);
      input('tp').classList.toggle('bad', !!ev.tp);
      const problem = ev.sl ? textFor('sl', ev.sl) : ev.tp ? textFor('tp', ev.tp) : '';
      const b = body(cur, n);
      msg.classList.toggle('ok', !problem && !serverMsg);
      msg.textContent = problem || serverMsg || (b ? '' : t('lv.nothing', 'Nothing changed yet'));
      save.disabled = busy || !!problem || !b;
      return { ev, b };
    }

    back.addEventListener('input', () => { serverMsg = ''; render(); });
    back.addEventListener('click', async (e) => {
      const el = /** @type {HTMLElement} */ (e.target);
      if (el === back) { close(); return; }
      const btn = /** @type {HTMLElement|null} */ (el.closest('[data-act]'));
      if (!btn) return;
      const act = btn.dataset.act;
      const k = /** @type {'sl'|'tp'} */ (btn.dataset.k);
      serverMsg = '';
      if (act === 'close') { close(); return; }
      if (act === 'clear') { input(k).value = ''; render(); return; }
      if (act === 'min') {
        const ref = o.ref();
        const sug = ref != null ? window.Stops.suggest(row.side, ref, o.bps() || 0, isDeal) : null;
        if (sug) input(k).value = inputVal(sug[k]);
        render();
        input(k).focus();
        return;
      }
      if (act === 'save') {
        const { b } = render();
        if (!b || save.disabled) return;
        busy = true; save.disabled = true;
        let res;
        try { res = await o.submit(b); } catch { res = { ok: false, status: 0, data: null }; }
        busy = false;
        if (!current || current.el !== back) return;
        if (res && res.ok) { close(); if (o.onDone) o.onDone(res.data); return; }
        const d = res ? res.data : null;
        const code = d && d.error;
        const near = d && Number.isInteger(d.limit) ? ' ' + t('f.nearest', 'Nearest allowed level: {px}', { px: fmt(d.limit) }) : '';
        serverMsg = (o.rejection ? o.rejection(res ? res.status : 0, code) : `Rejected (${res ? res.status : 0})`) + near;
        render();
      }
    });
    const onKey = (/** @type {KeyboardEvent} */ e) => {
      if (e.key === 'Escape') close();
      else if (e.key === 'Enter' && !save.disabled && /** @type {HTMLElement} */ (e.target).tagName === 'INPUT') save.click();
    };
    document.addEventListener('keydown', onKey);
    const timer = setInterval(render, 1000);
    current = {
      el: back,
      destroy() { clearInterval(timer); document.removeEventListener('keydown', onKey); back.remove(); },
    };
    render();
    const f = input(o.field === 'tp' ? 'tp' : 'sl');
    f.focus();
    f.select();
    return { close, el: back };
  }

  window.Levels = { open, close, evaluate, body, parse, isOpen: () => !!current };
})();
