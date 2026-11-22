'use strict';
(function () {
  const CSS = `
.watch { background: var(--panel,#10141d); border-right: 1px solid var(--border,#1f2735); display: flex; flex-direction: column; overflow: hidden; }
.watch__search { padding: 8px; }
.watch__search input { width: 100%; background: var(--panel2,#151a25); border: 1px solid var(--border,#1f2735); color: var(--text,#d7dce5);
  border-radius: 6px; padding: 7px 10px; }
.watch__title { padding: 6px 12px; color: var(--muted,#6b7688); font-weight: 700; font-size: 11px; letter-spacing: .3px; }
.watch__head, .irow { display: grid; grid-template-columns: 20px 1.25fr .78fr .82fr .82fr .62fr .82fr; gap: 4px; padding: 5px 10px; align-items: center; }
.star { color: #384253; cursor: pointer; display: inline-flex; }
.star svg { width: 13px; height: 13px; }
.star:hover { color: #7d8aa0; }
.star.on { color: var(--brand,#f0b90b); }
.star.on svg { fill: currentColor; }
.irow .high { text-align: right; color: var(--muted,#6b7688); }
.watch__head { color: var(--muted,#6b7688); font-size: 10px; border-bottom: 1px solid var(--border,#1f2735); background: var(--panel2,#151a25); }
.watch__rows { overflow-y: auto; }
.irow { border-bottom: 1px solid rgba(31,39,53,.5); cursor: pointer; font-variant-numeric: tabular-nums; }
.irow:hover { background: var(--panel2,#151a25); }
.irow.active { background: var(--panel3,#1a2130); box-shadow: inset 3px 0 0 var(--brand,#f0b90b); }
.irow .sym { font-weight: 600; } .irow .sym small { color: var(--muted,#6b7688); font-weight: 400; display: block; font-size: 10px; }
.irow .chg.up, .irow .c-last.up { color: var(--up,#26a69a); } .irow .chg.down, .irow .c-last.down { color: var(--down,#ef5350); }
.irow .sell { color: var(--down,#ef5350); } .irow .buy { color: var(--up,#26a69a); }
.star svg { width: 14px; height: 14px; }
.watch__head, .irow { grid-template-columns: 18px 1.55fr .82fr .85fr .85fr .7fr .85fr; }
.watch__head span { color: var(--muted,#6b7688); }
.sortable { cursor: pointer; user-select: none; display: inline-flex; align-items: center; gap: 3px; }
.sortable:hover { color: var(--text,#d7dce5); }
.sortable.asc i { border-left: 3px solid transparent; border-right: 3px solid transparent; border-bottom: 4px solid var(--brand,#f0b90b); }
.sortable.desc i { border-left: 3px solid transparent; border-right: 3px solid transparent; border-top: 4px solid var(--brand,#f0b90b); }
.irow { font-variant-numeric: tabular-nums; }
/* Ячейки не должны налезать на соседние колонки: при узкой панели длинные цены
   выходили за свой трек и сливались с соседями в кашу (BUG-009). */
.watch__head span, .irow .num, .irow .sym { overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.irow .num { text-align: left; color: var(--text,#d7dce5); }
.irow .num.muted { color: var(--muted,#6b7688); }
.irow .chg.up { color: var(--up,#26a69a); } .irow .chg.down { color: var(--down,#ef5350); }
.irow .sell { color: var(--down,#ef5350); } .irow .buy { color: var(--up,#26a69a); }
.sym { display: flex; align-items: center; gap: 7px; font-weight: 600; overflow: hidden; }
.symtxt { white-space: nowrap; } .symtxt small { color: var(--muted,#6b7688); font-weight: 400; font-size: 10px; }
.coinwrap { position: relative; width: 18px; height: 18px; flex: 0 0 18px; border-radius: 50%; overflow: hidden; }
.coinbadge { position: absolute; inset: 0; display: flex; align-items: center; justify-content: center; color: #fff; font-size: 9px; font-weight: 700; }
.coin { position: absolute; inset: 0; width: 100%; height: 100%; }
.irow .symtxt small { display: inline; }
.watch__head, .irow { grid-template-columns: var(--watch-cols, 18px 1.55fr .82fr .85fr .85fr .7fr .85fr); }
.watch.hide-last .c-last, .watch.hide-change .c-change, .watch.hide-sell .c-sell, .watch.hide-buy .c-buy,
.watch.hide-spread .c-spread, .watch.hide-high .c-high { display: none; }
.sym .symtxt { min-width: 0; overflow: hidden; text-overflow: ellipsis; }
.wl-mk { flex: 0 0 auto; max-width: 70%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 9.5px; font-weight: 500; line-height: 1;
  padding: 2px 4px; border-radius: 3px; color: var(--muted,#6b7688); border: 1px solid var(--border,#1f2735); }
.wl-mk--closed, .wl-mk--halted { color: var(--down,#ef5350); border-color: currentColor; }
.watch__cats { display: flex; gap: 4px; padding: 0 8px 6px; overflow-x: auto; scrollbar-width: none; flex: 0 0 auto; }
.watch__cats::-webkit-scrollbar { display: none; }
.watch__cats[hidden] { display: none; }
.wl-cat { flex: 0 0 auto; background: transparent; border: 1px solid var(--border,#1f2735); color: var(--muted,#6b7688); border-radius: 5px;
  padding: 4px 9px; font: inherit; font-size: 11px; font-weight: 600; cursor: pointer; white-space: nowrap; }
.wl-cat:hover { color: var(--text,#d7dce5); }
.wl-cat.on { color: var(--text,#d7dce5); background: var(--panel2,#151a25); border-color: var(--brand,#f0b90b); }
`;
  if (!document.getElementById('watchlist-css')) {
    const s = document.createElement('style'); s.id = 'watchlist-css'; s.textContent = CSS;
    document.head.appendChild(s);
  }

  const CATS = [
    { key: 'crypto', label: 'Crypto' },
    { key: 'forex', label: 'Forex' },
    { key: 'metals', label: 'Metals' },
    { key: 'indices', label: 'Indices' },
    { key: 'stocks', label: 'Stocks' },
    { key: 'energy', label: 'Energy' },
  ];
  /**
   * @param {any} it
   * @returns {string}
   */
  const catOf = (it) => (it && typeof it.category === 'string' && it.category ? it.category : 'crypto');
  /**
   * @param {string} c
   * @returns {number}
   */
  const catRank = (c) => { const i = CATS.findIndex((x) => x.key === c); return i < 0 ? CATS.length : i; };

  const STAR = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2"><path d="M8 2l1.8 3.9 4.2.4-3.2 2.9.9 4.1L8 11.3 4.3 13.2l.9-4.1L2 6.3l4.2-.4z" stroke-linejoin="round"/></svg>';

  const COLS = [
    { key: 'last', label: 'PRICE', width: '1fr', i18n: 'wl.price' },
    { key: 'change', label: 'CHANGE', width: '.82fr' },
    { key: 'sell', label: 'SELL', width: '.85fr' },
    { key: 'buy', label: 'BUY', width: '.85fr' },
    { key: 'spread', label: 'SPREAD', width: '.7fr' },
    { key: 'high', label: 'HIGH', width: '.85fr' },
  ];

  const SORT_METRIC = {
    last: (it) => it.last ?? -Infinity,
    change: (it) => it.change,
    sell: (it) => it.bid ?? -Infinity,
    buy: (it) => it.ask ?? -Infinity,
    spread: (it) => (it.ask != null && it.bid != null ? it.ask - it.bid : -Infinity),
    high: (it) => it.high ?? -Infinity,
  };

  function hue(s) {
    let h = 0;
    for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) % 360;
    return h;
  }

  function coinHtml(base) {
    base = String(base || '?');
    return `<span class="coinwrap"><span class="coinbadge" style="background:hsl(${hue(base)} 52% 40%)">${base[0]}</span>` +
      `<img class="coin" src="/vendor/coins/${base.toLowerCase()}.svg" onerror="this.remove()" alt=""></span>`;
  }

  function readJson(key, fallback) {
    try {
      return JSON.parse(localStorage.getItem(key)) ?? fallback;
    } catch (e) {
      console.warn(`[watchlist] настройка ${key} испорчена, взято умолчание`, e);
      return fallback;
    }
  }

  function mount(root, ctx) {
    const cfg = ctx || {};
    const prefix = cfg.storagePrefix || 'watch';
    const favsKey = `${prefix}.favs`;
    const colsKey = cfg.colsKey || `${prefix}.cols`;
    const catKey = `${prefix}.cat`;
    const t = cfg.t || ((k, en) => en);
    const num = cfg.num || ((/** @type {number} */ v, /** @type {number} */ n) => v.toLocaleString('en-US', { minimumFractionDigits: n, maximumFractionDigits: n }));
    const change = cfg.change || ((/** @type {number} */ v) => `${v >= 0 ? '+' : ''}${v.toFixed(2)}%`);

    const favs = new Set(readJson(favsKey, []));
    const cols = Object.assign({ last: 0, change: 1, sell: 1, buy: 1, spread: 1, high: 1 }, cfg.columnsDefault || {}, readJson(colsKey, {}));
    const rowEls = {};
    const meta = {};
    const order = {};
    let sortKey = null, sortDir = -1;
    let query = '';
    let cat = String(readJson(catKey, 'all') || 'all');
    let catsShown = '';
    let timer = null;

    root.innerHTML =
      `<div class="watch__search"><input class="wl-search" placeholder="${t('wl.search', 'Search…')}" data-i18n-ph="wl.search" /></div>` +
      `<div class="watch__cats" hidden></div>` +
      (cfg.title === '' ? '' : `<div class="watch__title">${cfg.title || '⭐ POPULAR'}</div>`) +
      `<div class="watch__head"><span></span><span data-i18n="wl.instr">${t('wl.instr', 'INSTR.')}</span>` +
      COLS.map((c) => { const k = c.i18n || `wl.${c.key}`; return `<span class="sortable c-${c.key}" data-sort="${c.key}"><span data-i18n="${k}">${t(k, c.label)}</span><i></i></span>`; }).join('') +
      `</div><div class="watch__rows"></div>`;

    const searchEl = root.querySelector('.wl-search');
    const headEl = root.querySelector('.watch__head');
    const rowsEl = root.querySelector('.watch__rows');
    const catsEl = root.querySelector('.watch__cats');

    function applyCols() {
      const fr = (w) => `minmax(0, ${w})`;
      const tracks = ['18px', fr('1.55fr'), ...COLS.filter((c) => cols[c.key]).map((c) => fr(c.width))];
      root.style.setProperty('--watch-cols', tracks.join(' '));
      COLS.forEach((c) => root.classList.toggle('hide-' + c.key, !cols[c.key]));
    }

    function reorder() {
      Object.keys(rowEls).map(Number).sort((a, b) => {
        const fa = favs.has(a), fb = favs.has(b);
        if (fa !== fb) return Number(fb) - Number(fa);
        if (!sortKey && meta[a] && meta[b]) {
          const c = catRank(catOf(meta[a])) - catRank(catOf(meta[b]));
          if (c) return c;
        }
        if (sortKey && meta[a] && meta[b]) {
          const m = SORT_METRIC[sortKey];
          const d = (m(meta[a]) - m(meta[b])) * sortDir;
          if (d) return d;
        }
        return (order[a] ?? a) - (order[b] ?? b);
      }).forEach((id) => rowsEl.appendChild(rowEls[id]));
    }

    function toggleFav(id) {
      if (favs.has(id)) favs.delete(id);
      else favs.add(id);
      localStorage.setItem(favsKey, JSON.stringify([...favs]));
      rowEls[id]?.querySelector('.star')?.classList.toggle('on', favs.has(id));
      reorder();
    }

    function applyFilter() {
      const byCat = !query && cat !== 'all' && catsShown !== '';
      Object.entries(rowEls).forEach(([id, el]) => {
        const sym = `${meta[id]?.symbol || ''} ${meta[id]?.name || ''}`.toLowerCase();
        const visible = sym.includes(query) && (!byCat || catOf(meta[id]) === cat);
        el.style.display = visible ? '' : 'none';
      });
    }

    function renderCats() {
      const present = [...new Set(Object.values(meta).map(catOf))].sort((a, b) => catRank(a) - catRank(b));
      const sig = present.length > 1 ? present.join(',') : '';
      if (sig && cat !== 'all' && !present.includes(cat)) cat = 'all';
      if (sig === catsShown) {
        catsEl.querySelectorAll('.wl-cat').forEach((b) => b.classList.toggle('on', /** @type {HTMLElement} */ (b).dataset.cat === cat));
        return;
      }
      catsShown = sig;
      catsEl.hidden = !sig;
      if (!sig) { catsEl.innerHTML = ''; return; }
      const label = (/** @type {string} */ k) => { const c = CATS.find((x) => x.key === k); return t(`wl.cat.${k}`, c ? c.label : k); };
      catsEl.innerHTML = ['all', ...present].map((k) =>
        `<button type="button" class="wl-cat${k === cat ? ' on' : ''}" data-cat="${k}" data-i18n="wl.cat.${k}">${k === 'all' ? t('wl.cat.all', 'All') : label(k)}</button>`).join('');
    }

    /** @param {string} next */
    function setCat(next) {
      cat = next;
      try { localStorage.setItem(catKey, JSON.stringify(cat)); } catch {  }
      renderCats();
      applyFilter();
    }

    function fmtPrice(it, raw) {
      const d = it.price_decimals ?? 2;
      return num(raw / 10 ** d, d);
    }

    /**
     * @param {any} it
     * @returns {[string, string]}
     */
    function pairOf(it) {
      return window.Market ? window.Market.pair(it) : [String(it.symbol), ''];
    }

    /** @param {any} it */
    function noteHtml(it) {
      const n = cfg.note ? cfg.note(it) : null;
      if (!n) return '';
      return `<span class="wl-mk wl-mk--${n.kind}" title="${n.tip || n.text}">${n.text}</span>`;
    }

    function renderRow(el, it) {
      const known = it.change != null;
      const up = known && it.change >= 0;
      const [base, quote] = pairOf(it);
      el.title = it.name || '';
      el.innerHTML =
        `<span class="star ${favs.has(it.id) ? 'on' : ''}">${STAR}</span>` +
        `<div class="sym">${coinHtml(base)}<span class="symtxt">${base}${quote ? `<small>/${quote}</small>` : ''}</span>${noteHtml(it)}</div>` +
        `<div class="num c-last ${known ? (up ? 'up' : 'down') : ''}">${it.last != null ? fmtPrice(it, it.last) : '—'}</div>` +
        `<div class="num chg c-change ${known ? (up ? 'up' : 'down') : ''}">${known ? change(it.change) : '—'}</div>` +
        `<div class="num sell c-sell">${it.bid != null ? fmtPrice(it, it.bid) : '—'}</div>` +
        `<div class="num buy c-buy">${it.ask != null ? fmtPrice(it, it.ask) : '—'}</div>` +
        `<div class="num muted c-spread">${it.bid != null && it.ask != null ? it.ask - it.bid : '—'}</div>` +
        `<div class="num c-high">${it.high != null ? fmtPrice(it, it.high) : '—'}</div>`;
    }

    async function refresh() {
      const list = cfg.fetch ? await cfg.fetch() : null;
      if (!list) return;
      const current = cfg.instrument ? cfg.instrument() : null;
      let firstId = null;
      const seen = new Set(list.map((it) => String(it.id)));
      Object.keys(rowEls).forEach((id) => {
        if (seen.has(id)) return;
        rowEls[id].remove();
        delete rowEls[id];
        delete meta[id];
        delete order[id];
      });
      list.forEach((it, i) => { order[it.id] = i; });
      for (const it of list) {
        meta[it.id] = it;
        if (firstId === null) firstId = it.id;
        let el = rowEls[it.id];
        if (!el) {
          el = document.createElement('div');
          el.className = 'irow';
          el.dataset.id = it.id;
          rowsEl.appendChild(el);
          rowEls[it.id] = el;
        }
        renderRow(el, it);
        el.classList.toggle('active', it.id === current);
      }
      renderCats();
      reorder();
      applyFilter();
      if (cfg.onData) cfg.onData(list);
      if (current == null && firstId != null && cfg.onSelect) {
        const wanted = cfg.initial ? cfg.initial(list) : null;
        cfg.onSelect(wanted != null && meta[wanted] ? wanted : firstId);
      }
    }

    /**
     * @param {number} id
     * @param {object} patch
     */
    function update(id, patch) {
      const it = meta[id], el = rowEls[id];
      if (!it || !el) return;
      Object.assign(it, patch);
      renderRow(el, it);
    }

    function setActive(id) {
      Object.entries(rowEls).forEach(([k, el]) => el.classList.toggle('active', +k === id));
    }

    const onRowsClick = (e) => {
      const row = e.target.closest('.irow');
      if (!row) return;
      const id = +row.dataset.id;
      if (e.target.closest('.star')) {
        toggleFav(id);
        return;
      }
      setActive(id);
      if (cfg.onSelect) cfg.onSelect(id);
    };
    const onHeadClick = (e) => {
      const h = e.target.closest('.sortable');
      if (!h) return;
      const k = h.dataset.sort;
      if (sortKey === k) sortDir = -sortDir;
      else { sortKey = k; sortDir = -1; }
      headEl.querySelectorAll('.sortable').forEach((x) => x.classList.remove('asc', 'desc'));
      h.classList.add(sortDir === 1 ? 'asc' : 'desc');
      reorder();
    };
    const onSearch = (e) => { query = e.target.value.toLowerCase(); applyFilter(); };
    const onCatClick = (e) => {
      const b = e.target.closest('.wl-cat');
      if (b) setCat(b.dataset.cat);
    };

    rowsEl.addEventListener('click', onRowsClick);
    headEl.addEventListener('click', onHeadClick);
    searchEl.addEventListener('input', onSearch);
    catsEl.addEventListener('click', onCatClick);

    applyCols();
    refresh();
    if (cfg.interval) timer = setInterval(refresh, cfg.interval);

    return {
      refresh,
      setActive,
      update,
      category: () => cat,
      setCategory: setCat,
      columns: () => ({ ...cols }),
      setColumns(next) {
        Object.assign(cols, next);
        localStorage.setItem(colsKey, JSON.stringify(cols));
        applyCols();
      },
      destroy() {
        if (timer) clearInterval(timer);
        rowsEl.removeEventListener('click', onRowsClick);
        headEl.removeEventListener('click', onHeadClick);
        searchEl.removeEventListener('input', onSearch);
        catsEl.removeEventListener('click', onCatClick);
        root.innerHTML = '';
      },
    };
  }

  window.Watchlist = { mount };
})();
