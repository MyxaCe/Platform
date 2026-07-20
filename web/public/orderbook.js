/* ============================================================================
 * orderbook.js — биржевой стакан (ADR-015): свой DOM, свои стили, свои настройки.
 *
 *   const book = OrderBook.mount(el, {
 *     instrument: () => currentId,
 *     fetch: (id) => Promise<Depth>,          // { price_decimals, qty_decimals, bids, asks }
 *     onPick: (priceStr) => {...},            // клик по уровню
 *     symbols: (id) => ({ base, quote }),     // подписи колонок
 *     markPrice: (id) => number|null,         // цена под серединой (в единицах, не raw)
 *     interval: 700, levels: 20, storagePrefix: 'ob',
 *   });
 *
 * Возможности: группировка цен по шагу (шаг тика ×1/×10/×100), режимы показа
 * (обе стороны / только покупки / только продажи), глубина полос по объёму строки
 * или нарастающим итогом, маркер средней суммы, полоса соотношения B/S, анимация.
 * Настройки живут в localStorage под своим префиксом и правятся через меню «⋯».
 * ========================================================================== */
(function () {
  const CSS = `
.ob { display: flex; flex-direction: column; min-height: 0; height: 100%; background: var(--panel,#10141d); }
.ob__head { display: flex; align-items: center; justify-content: space-between; padding: 9px 12px;
  font-size: 12px; font-weight: 700; border-bottom: 1px solid var(--border,#1f2735); }
.ob__dots { background: none; border: none; color: var(--muted,#6b7688); cursor: pointer; font-size: 15px;
  line-height: 1; padding: 2px 6px; border-radius: 4px; font-family: inherit; }
.ob__dots:hover { background: var(--panel2,#151a25); color: var(--text,#d7dce5); }
.ob__wrap { position: relative; display: flex; flex-direction: column; flex: 1; min-height: 0; }

/* Меню настроек */
.ob__menu { position: absolute; right: 8px; top: 4px; z-index: 20; width: 232px; padding: 8px;
  background: var(--panel2,#151a25); border: 1px solid var(--border,#1f2735); border-radius: 8px;
  box-shadow: 0 8px 24px rgba(0,0,0,.5); font-weight: 400; }
.ob__menu.hidden { display: none; }
.ob__cat { color: var(--muted,#6b7688); font-size: 10px; letter-spacing: .5px; text-transform: uppercase; padding: 6px 4px 4px; }
.ob__opt { display: flex; align-items: center; justify-content: space-between; gap: 8px;
  padding: 5px 4px; font-size: 12px; cursor: pointer; border-radius: 5px; }
.ob__opt:hover { background: var(--panel3,#1a2130); }
.ob__opt input { accent-color: var(--accent,#f0b90b); cursor: pointer; }

/* Режимы показа + группировка */
.ob__ctl { display: flex; align-items: center; justify-content: space-between; padding: 6px 10px;
  border-bottom: 1px solid var(--border,#1f2735); }
.ob__modes { display: flex; gap: 4px; }
.ob__mode { width: 22px; height: 22px; padding: 0; display: flex; align-items: center; justify-content: center;
  background: transparent; border: 1px solid transparent; border-radius: 4px; cursor: pointer; }
.ob__mode:hover { background: var(--panel2,#151a25); }
.ob__mode.active { border-color: var(--accent,#f0b90b); }
.ob__mode i { display: block; width: 12px; height: 12px; position: relative; }
.ob__mode i::before, .ob__mode i::after { content: ''; position: absolute; left: 0; right: 0; height: 5px; border-radius: 1px; }
.ob__mode i::before { top: 0; background: var(--down,#ef5350); }
.ob__mode i::after { bottom: 0; background: var(--up,#26a69a); }
.ob__mode[data-m="asks"] i::after { display: none; }
.ob__mode[data-m="asks"] i::before { top: 0; bottom: 0; height: auto; }
.ob__mode[data-m="bids"] i::before { display: none; }
.ob__mode[data-m="bids"] i::after { top: 0; bottom: 0; height: auto; }
.ob__group { background: var(--panel2,#151a25); border: 1px solid var(--border,#1f2735); color: var(--text,#d7dce5);
  border-radius: 5px; padding: 2px 6px; font-size: 11px; font-family: inherit; cursor: pointer; }

.ob__cols { display: grid; grid-template-columns: 1fr 1fr 1fr; gap: 4px; padding: 5px 10px;
  color: var(--muted,#6b7688); font-size: 10px; }
.ob__cols span:nth-child(n+2) { text-align: right; }

.ob__side { flex: 1 1 0; overflow: hidden; display: flex; flex-direction: column; min-height: 0; position: relative; }
.ob__side.asks { justify-content: flex-end; }
/* В одностороннем режиме вторая сторона убирается совсем, иначе она держала бы
   половину высоты пустой, и книга занимала бы только половину области. */
.ob__side.off { display: none; }
.brow { position: relative; display: grid; grid-template-columns: 1fr 1fr 1fr; gap: 4px; padding: 2px 10px;
  font-variant-numeric: tabular-nums; font-size: 11px; cursor: pointer; }
.brow:hover { background: rgba(255,255,255,.04); }
.brow .bar { position: absolute; top: 0; bottom: 0; right: 0; }
.ob--anim .brow .bar { transition: width .18s ease-out; }
.brow.ask .bar { background: rgba(239,83,80,.16); } .brow.bid .bar { background: rgba(38,166,154,.16); }
.brow.ask .bp { color: var(--down,#ef5350); } .brow.bid .bp { color: var(--up,#26a69a); }
.brow span { position: relative; z-index: 1; }
.brow span:nth-child(n+3) { text-align: right; }
.brow .bq { text-align: right; }
/* Маркер средней суммы: вертикальная риска на полосах */
.ob__avg { position: absolute; top: 0; bottom: 0; width: 1px; background: var(--accent,#f0b90b);
  opacity: .5; pointer-events: none; z-index: 2; }

.ob__mid { display: flex; align-items: center; gap: 8px; padding: 7px 10px; font-variant-numeric: tabular-nums;
  border-top: 1px solid var(--border,#1f2735); border-bottom: 1px solid var(--border,#1f2735); }
.ob__last { font-size: 17px; font-weight: 700; }
.ob__last.up { color: var(--up,#26a69a); } .ob__last.down { color: var(--down,#ef5350); }
.ob__mark { color: var(--muted,#6b7688); font-size: 12px; }
.ob__spread { margin-left: auto; color: var(--muted,#6b7688); font-size: 11px; }

.ob__ratio { display: flex; align-items: center; gap: 6px; padding: 6px 10px; font-size: 11px;
  font-weight: 700; font-variant-numeric: tabular-nums; border-top: 1px solid var(--border,#1f2735); }
.ob__ratio.hidden { display: none; }
.ob__ratio .b { color: var(--up,#26a69a); } .ob__ratio .s { color: var(--down,#ef5350); }
.ob__bar { flex: 1; height: 8px; display: flex; border-radius: 2px; overflow: hidden; background: var(--down,#ef5350); }
.ob__bar i { display: block; height: 100%; background: var(--up,#26a69a); }
.ob--anim .ob__bar i { transition: width .25s ease-out; }
`;
  if (!document.getElementById('orderbook-css')) {
    const s = document.createElement('style'); s.id = 'orderbook-css'; s.textContent = CSS;
    document.head.appendChild(s);
  }

  const DEFAULTS = { avg: false, ratio: true, rounding: true, depth: 'sum', anim: true, mode: 'both', groupMul: 1 };

  /** Компактный формат для колонки «Всего»: 2 090 → 2.09K. */
  function compact(v) {
    const a = Math.abs(v);
    if (a >= 1e9) return (v / 1e9).toFixed(2) + 'B';
    if (a >= 1e6) return (v / 1e6).toFixed(2) + 'M';
    if (a >= 1e3) return (v / 1e3).toFixed(2) + 'K';
    return v.toFixed(2);
  }

  function mount(root, ctx) {
    const levels = ctx.levels || 20;
    const prefix = ctx.storagePrefix || 'ob';
    const key = `${prefix}.settings`;
    let cfg = Object.assign({}, DEFAULTS, JSON.parse(localStorage.getItem(key) || '{}'));
    const save = () => localStorage.setItem(key, JSON.stringify(cfg));

    root.classList.add('ob');
    root.innerHTML = `
      <div class="ob__head">Order Book<button class="ob__dots" title="Settings">⋯</button></div>
      <div class="ob__wrap">
        <div class="ob__menu hidden">
          <div class="ob__cat">Order book display</div>
          <label class="ob__opt"><span>Show average amount</span><input type="checkbox" data-k="avg"></label>
          <label class="ob__opt"><span>Show buy/sell ratio</span><input type="checkbox" data-k="ratio"></label>
          <label class="ob__opt"><span>Rounding</span><input type="checkbox" data-k="rounding"></label>
          <div class="ob__cat">Visual depth</div>
          <label class="ob__opt"><span>Amount</span><input type="radio" name="${prefix}-depth" data-d="sum"></label>
          <label class="ob__opt"><span>Cumulative</span><input type="radio" name="${prefix}-depth" data-d="cum"></label>
          <div class="ob__cat">Animation</div>
          <label class="ob__opt"><span>Enabled</span><input type="checkbox" data-k="anim"></label>
        </div>
        <div class="ob__ctl">
          <div class="ob__modes">
            <button class="ob__mode" data-m="both" title="Bids and asks"><i></i></button>
            <button class="ob__mode" data-m="bids" title="Bids only"><i></i></button>
            <button class="ob__mode" data-m="asks" title="Asks only"><i></i></button>
          </div>
          <select class="ob__group" title="Price grouping"></select>
        </div>
        <div class="ob__cols"><span class="c-price">Price</span><span class="c-amt">Amount</span><span>Total</span></div>
        <div class="ob__side asks"></div>
        <div class="ob__mid"><span class="ob__last">—</span><span class="ob__mark"></span><span class="ob__spread"></span></div>
        <div class="ob__side bids"></div>
        <div class="ob__ratio"><span class="b">B</span><span class="bv">—</span><div class="ob__bar"><i style="width:50%"></i></div><span class="sv">—</span><span class="s">S</span></div>
      </div>`;

    const $ = (s) => root.querySelector(s);
    const elAsks = $('.asks'), elBids = $('.bids'), elMenu = $('.ob__menu'), elGroup = $('.ob__group');
    const elLast = $('.ob__last'), elMark = $('.ob__mark'), elSpread = $('.ob__spread');
    const elRatio = $('.ob__ratio'), elBar = $('.ob__bar i'), elBv = $('.bv'), elSv = $('.sv');

    // ---- Настройки ----------------------------------------------------------
    $('.ob__dots').addEventListener('click', (e) => { e.stopPropagation(); elMenu.classList.toggle('hidden'); });
    document.addEventListener('click', (e) => { if (!elMenu.contains(e.target)) elMenu.classList.add('hidden'); });
    elMenu.addEventListener('change', (e) => {
      const t = e.target;
      if (t.dataset.k) cfg[t.dataset.k] = t.checked;
      if (t.dataset.d && t.checked) cfg.depth = t.dataset.d;
      save(); reload();
    });
    root.querySelectorAll('.ob__mode').forEach((b) => b.addEventListener('click', () => {
      cfg.mode = b.dataset.m; save(); reload();
    }));
    elGroup.addEventListener('change', () => { cfg.groupMul = +elGroup.value; save(); reload(); });

    function applyCfg() {
      elMenu.querySelectorAll('[data-k]').forEach((i) => { i.checked = !!cfg[i.dataset.k]; });
      elMenu.querySelectorAll('[data-d]').forEach((i) => { i.checked = cfg.depth === i.dataset.d; });
      root.querySelectorAll('.ob__mode').forEach((b) => b.classList.toggle('active', b.dataset.m === cfg.mode));
      elAsks.classList.toggle('off', cfg.mode === 'bids');
      elBids.classList.toggle('off', cfg.mode === 'asks');
      root.classList.toggle('ob--anim', !!cfg.anim);
      elRatio.classList.toggle('hidden', !cfg.ratio);
      elGroup.style.display = cfg.rounding ? '' : 'none';
    }

    // ---- Данные -------------------------------------------------------------
    let last = null;        // последний ответ /depth
    let prevMid = null;     // предыдущая середина
    // Направление «липкое»: цвет держится до следующего изменения цены. Иначе между
    // опросами цена часто совпадает, класс снимался и строка была белой почти всегда.
    let dir = null;

    /** Свести уровни к шагу `step` (в raw-единицах цены). */
    function group(list, step, isBid) {
      if (step <= 1) return list;
      const out = new Map();
      for (const [p, q] of list) {
        const k = isBid ? Math.floor(p / step) * step : Math.ceil(p / step) * step;
        out.set(k, (out.get(k) || 0) + q);
      }
      const arr = [...out.entries()];
      arr.sort((a, b) => (isBid ? b[0] - a[0] : a[0] - b[0]));
      return arr;
    }

    function render() {
      const d = last; if (!d) return;
      const id = ctx.instrument();
      const pd = d.price_decimals, qd = d.qty_decimals;
      const ps = 10 ** pd, qs = 10 ** qd;
      const step = cfg.rounding ? cfg.groupMul : 1; // groupMul в тиках

      const asksAll = group(d.asks, step, false);
      const bidsAll = group(d.bids, step, true);
      const n = cfg.mode === 'both' ? levels : levels * 2; // одна сторона — вдвое больше строк
      const asks = cfg.mode === 'bids' ? [] : asksAll.slice(0, n);
      const bids = cfg.mode === 'asks' ? [] : bidsAll.slice(0, n);

      // Ширина полос: от объёма строки либо нарастающим итогом.
      const cum = (arr) => { let s = 0; return arr.map(([p, q]) => { s += q; return [p, q, s]; }); };
      const asksC = cum(asks), bidsC = cum(bids);
      const maxRow = Math.max(1, ...asks.map((x) => x[1]), ...bids.map((x) => x[1]));
      const maxCum = Math.max(1, asksC.length ? asksC[asksC.length - 1][2] : 1, bidsC.length ? bidsC[bidsC.length - 1][2] : 1);
      const width = (q, c) => (cfg.depth === 'cum' ? (c / maxCum) : (q / maxRow)) * 100;

      const row = ([p, q, c], cls) => {
        const price = (p / ps).toFixed(pd);
        return `<div class="brow ${cls}" data-p="${price}"><div class="bar" style="width:${width(q, c).toFixed(1)}%"></div>` +
          `<span class="bp">${price}</span><span class="bq">${(q / qs).toFixed(3)}</span><span class="bt">${compact((p / ps) * (q / qs))}</span></div>`;
      };
      elAsks.innerHTML = asksC.slice().reverse().map((r) => row(r, 'ask')).join('');
      elBids.innerHTML = bidsC.map((r) => row(r, 'bid')).join('');

      // Маркер средней суммы по видимым уровням.
      root.querySelectorAll('.ob__avg').forEach((e) => e.remove());
      if (cfg.avg) {
        const all = [...asks, ...bids];
        if (all.length) {
          const avg = all.reduce((s, x) => s + x[1], 0) / all.length;
          for (const side of [elAsks, elBids]) {
            const m = document.createElement('div');
            m.className = 'ob__avg';
            m.style.right = `${Math.min(100, (avg / maxRow) * 100)}%`;
            side.appendChild(m);
          }
        }
      }

      // Середина: цена, направление, марк-цена, спред.
      const bb = bidsAll[0]?.[0], ba = asksAll[0]?.[0];
      if (bb && ba) {
        const mid = (bb + ba) / 2 / ps;
        if (prevMid != null && mid !== prevMid) dir = mid > prevMid ? 'up' : 'down';
        if (mid !== prevMid) prevMid = mid;
        elLast.textContent = mid.toFixed(pd);
        elLast.classList.toggle('up', dir === 'up');
        elLast.classList.toggle('down', dir === 'down');
        elSpread.textContent = `spread ${((ba - bb) / ps).toFixed(pd)}`;
      } else {
        elLast.textContent = '—'; elSpread.textContent = '';
      }
      const mark = ctx.markPrice && ctx.markPrice(id);
      elMark.textContent = mark != null ? `$${mark.toFixed(pd)}` : '';

      // Полоса соотношения: доли суммарного объёма покупок и продаж.
      const sumB = bidsAll.slice(0, levels).reduce((s, x) => s + x[1], 0);
      const sumA = asksAll.slice(0, levels).reduce((s, x) => s + x[1], 0);
      const tot = sumB + sumA;
      const pb = tot ? (sumB / tot) * 100 : 50;
      elBar.style.width = `${pb.toFixed(2)}%`;
      elBv.textContent = `${pb.toFixed(2)}%`;
      elSv.textContent = `${(100 - pb).toFixed(2)}%`;

      // Подписи колонок с валютами инструмента.
      const sym = (ctx.symbols && ctx.symbols(id)) || {};
      $('.c-price').textContent = sym.quote ? `Price (${sym.quote})` : 'Price';
      $('.c-amt').textContent = sym.base ? `Amount (${sym.base})` : 'Amount';

      // Варианты группировки: шаг тика ×1/×10/×100.
      const want = [1, 10, 100].map((m) => `${m}|${(m / ps).toFixed(Math.max(0, pd))}`).join(',');
      if (elGroup.dataset.built !== want) {
        elGroup.dataset.built = want;
        elGroup.innerHTML = [1, 10, 100].map((m) => `<option value="${m}">${(m / ps).toFixed(Math.max(0, pd))}</option>`).join('');
        elGroup.value = String(cfg.groupMul);
      }
    }

    root.addEventListener('click', (e) => {
      const r = e.target.closest('.brow');
      if (r && r.dataset.p && ctx.onPick) ctx.onPick(r.dataset.p);
    });

    /** Сколько уровней просить у сервера: грубый шаг группировки съедает их пачками,
     *  а односторонний режим показывает вдвое больше строк. */
    function needLevels() {
      const rows = cfg.mode === 'both' ? levels : levels * 2;
      const mul = cfg.rounding ? cfg.groupMul : 1;
      return Math.min(1000, Math.max(20, rows * mul));
    }

    async function refresh() {
      const id = ctx.instrument(); if (id == null) return;
      let d; try { d = await ctx.fetch(id, needLevels()); } catch { return; }
      if (!d || !d.asks) return;
      last = d;
      render();
    }

    /** Настройка изменилась — перечитать с нужной глубиной, а не ждать тика. */
    function reload() { applyCfg(); refresh(); }

    applyCfg();
    refresh();
    const timer = ctx.interval ? setInterval(refresh, ctx.interval) : null;
    return {
      refresh,
      /** Смена инструмента: стрелка направления от чужой цены смысла не имеет. */
      reset() { prevMid = null; dir = null; last = null; },
      destroy() { if (timer) clearInterval(timer); root.innerHTML = ''; },
    };
  }

  window.OrderBook = { mount };
})();
