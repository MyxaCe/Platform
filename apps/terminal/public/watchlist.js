'use strict';
// ============================================================================
// watchlist.js — виджет списка инструментов (ADR-015, контракт mount(root, ctx)).
//
// Отвечает за: поиск, заголовок с сортировкой, строки инструментов, избранное,
// видимость колонок. Строит весь свой DOM внутри root — страница даёт пустой
// контейнер и не знает про внутренние элементы.
//
// Форматирует цены сам: точность приходит в самих данных (price_decimals),
// поэтому внешний форматтер не нужен — на одну связь меньше.
//
//   const wl = Watchlist.mount(root, {
//     fetch:      () => api.instruments(),   // источник данных (инъекция)
//     instrument: () => selected,            // какой инструмент подсвечен
//     onSelect:   (id) => {…},               // пользователь кликнул строку
//     onData:     (list) => {…},             // список обновился (композитор кэширует META)
//     interval:   1000,                      // автообновление, мс (0 — выключить)
//     title:      '⭐ POPULAR',
//     storagePrefix: 'watch',                // ключи localStorage: <prefix>.favs / <prefix>.cols
//   });
//   wl.refresh(); wl.columns(); wl.setColumns({ spread: 0 }); wl.destroy();
// ============================================================================
(function () {
  // Стили компонента едут вместе с модулем (ADR-015): страница-хост может не иметь своей CSS.
  // Токены темы (--panel/--border/--up/--down/--accent/...) — контракт хоста, у каждого фолбэк.
  // Порядок правил сохранён как в исходном style.css: поздние намеренно переопределяют ранние
  // (ширины колонок задаются трижды, последнее слово за `--watch-cols`).
  const CSS = `
.watch { background: var(--panel,#10141d); border-right: 1px solid var(--border,#1f2735); display: flex; flex-direction: column; overflow: hidden; }
.watch__search { padding: 8px; }
.watch__search input { width: 100%; background: var(--panel2,#151a25); border: 1px solid var(--border,#1f2735); color: var(--text,#d7dce5);
  border-radius: 6px; padding: 7px 10px; }
.watch__title { padding: 6px 12px; color: var(--brand,#f0b90b); font-weight: 700; font-size: 11px; letter-spacing: .5px; }
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
.irow .chg.up { color: var(--up,#26a69a); } .irow .chg.down { color: var(--down,#ef5350); }
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
.watch.hide-change .c-change, .watch.hide-sell .c-sell, .watch.hide-buy .c-buy,
.watch.hide-spread .c-spread, .watch.hide-high .c-high { display: none; }
`;
  if (!document.getElementById('watchlist-css')) {
    const s = document.createElement('style'); s.id = 'watchlist-css'; s.textContent = CSS;
    document.head.appendChild(s);
  }

  const STAR = '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2"><path d="M8 2l1.8 3.9 4.2.4-3.2 2.9.9 4.1L8 11.3 4.3 13.2l.9-4.1L2 6.3l4.2-.4z" stroke-linejoin="round"/></svg>';

  /** Колонки: ключ → ширина трека в CSS-гриде. Порядок задаёт порядок в строке. */
  const COLS = [
    { key: 'change', label: 'CHANGE', width: '.82fr' },
    { key: 'sell', label: 'SELL', width: '.85fr' },
    { key: 'buy', label: 'BUY', width: '.85fr' },
    { key: 'spread', label: 'SPREAD', width: '.7fr' },
    { key: 'high', label: 'HIGH', width: '.85fr' },
  ];

  /** Как достать из инструмента величину для сортировки. `-Infinity` уводит пустые вниз. */
  const SORT_METRIC = {
    change: (it) => it.change,
    sell: (it) => it.bid ?? -Infinity,
    buy: (it) => it.ask ?? -Infinity,
    spread: (it) => (it.ask != null && it.bid != null ? it.ask - it.bid : -Infinity),
    high: (it) => it.high ?? -Infinity,
  };

  /** Стабильный цвет бейджа по тикеру — заглушка, пока не загрузилось лого монеты. */
  function hue(s) {
    let h = 0;
    for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) % 360;
    return h;
  }

  function coinHtml(base) {
    return `<span class="coinwrap"><span class="coinbadge" style="background:hsl(${hue(base)} 52% 40%)">${base[0]}</span>` +
      `<img class="coin" src="/vendor/coins/${base.toLowerCase()}.svg" onerror="this.remove()" alt=""></span>`;
  }

  function readJson(key, fallback) {
    try {
      return JSON.parse(localStorage.getItem(key)) ?? fallback;
    } catch {
      return fallback;
    }
  }

  function mount(root, ctx) {
    const cfg = ctx || {};
    const prefix = cfg.storagePrefix || 'watch';
    const favsKey = `${prefix}.favs`;
    const colsKey = `${prefix}.cols`;

    const favs = new Set(readJson(favsKey, []));
    const cols = Object.assign({ change: 1, sell: 1, buy: 1, spread: 1, high: 1 }, readJson(colsKey, {}));
    const rowEls = {};       // id → элемент строки (переиспользуем, чтобы не терять скролл)
    const meta = {};         // id → последний известный инструмент
    let sortKey = null, sortDir = -1;
    let query = '';
    let timer = null;

    // ---- DOM (строим сами) -------------------------------------------------
    root.innerHTML =
      `<div class="watch__search"><input class="wl-search" placeholder="Search…" /></div>` +
      `<div class="watch__title">${cfg.title || '⭐ POPULAR'}</div>` +
      `<div class="watch__head"><span></span><span>INSTR.</span>` +
      COLS.map((c) => `<span class="sortable c-${c.key}" data-sort="${c.key}">${c.label}<i></i></span>`).join('') +
      `</div><div class="watch__rows"></div>`;

    const searchEl = root.querySelector('.wl-search');
    const headEl = root.querySelector('.watch__head');
    const rowsEl = root.querySelector('.watch__rows');

    // ---- Колонки -----------------------------------------------------------
    function applyCols() {
      // minmax(0, …) обязателен: у голого `Nfr` минимум равен min-content, поэтому длинные
      // цены (0.00000413) распирали числовые колонки и выдавливали колонку тикера в 0px —
      // текст вылезал поверх соседей, шапка расходилась со строками. См. bug-log BUG-009.
      const fr = (w) => `minmax(0, ${w})`;
      const tracks = ['18px', fr('1.55fr'), ...COLS.filter((c) => cols[c.key]).map((c) => fr(c.width))];
      root.style.setProperty('--watch-cols', tracks.join(' '));
      COLS.forEach((c) => root.classList.toggle('hide-' + c.key, !cols[c.key]));
    }

    // ---- Порядок строк: избранное сверху, затем сортировка, затем по id ----
    function reorder() {
      Object.keys(rowEls).map(Number).sort((a, b) => {
        const fa = favs.has(a), fb = favs.has(b);
        if (fa !== fb) return fb - fa;
        if (sortKey && meta[a] && meta[b]) {
          const m = SORT_METRIC[sortKey];
          const d = (m(meta[a]) - m(meta[b])) * sortDir;
          if (d) return d;
        }
        return a - b;
      }).forEach((id) => rowsEl.appendChild(rowEls[id]));
    }

    function toggleFav(id) {
      if (favs.has(id)) favs.delete(id);
      else favs.add(id);
      localStorage.setItem(favsKey, JSON.stringify([...favs]));
      rowEls[id]?.querySelector('.star')?.classList.toggle('on', favs.has(id));
      reorder();
    }

    /** Фильтр поиска применяется к уже существующим строкам — без перестроения DOM. */
    function applyFilter() {
      Object.entries(rowEls).forEach(([id, el]) => {
        const sym = (meta[id]?.symbol || '').toLowerCase();
        el.style.display = sym.includes(query) ? '' : 'none';
      });
    }

    function fmtPrice(it, raw) {
      const d = it.price_decimals ?? 2;
      return (raw / 10 ** d).toLocaleString('en-US', { minimumFractionDigits: d, maximumFractionDigits: d });
    }

    function renderRow(el, it) {
      const up = it.change >= 0;
      const [base, quote] = it.symbol.split('-');
      el.innerHTML =
        `<span class="star ${favs.has(it.id) ? 'on' : ''}">${STAR}</span>` +
        `<div class="sym">${coinHtml(base)}<span class="symtxt">${base}<small>/${quote || 'USDT'}</small></span></div>` +
        `<div class="num chg c-change ${up ? 'up' : 'down'}">${up ? '+' : ''}${it.change.toFixed(2)}%</div>` +
        `<div class="num sell c-sell">${it.bid != null ? fmtPrice(it, it.bid) : '—'}</div>` +
        `<div class="num buy c-buy">${it.ask != null ? fmtPrice(it, it.ask) : '—'}</div>` +
        `<div class="num muted c-spread">${it.bid != null && it.ask != null ? it.ask - it.bid : '—'}</div>` +
        `<div class="num c-high">${it.high != null ? fmtPrice(it, it.high) : '—'}</div>`;
    }

    async function refresh() {
      const list = cfg.fetch ? await cfg.fetch() : null;
      if (!list) return;               // сеть недоступна — оставляем прежние строки
      const current = cfg.instrument ? cfg.instrument() : null;
      let firstId = null;
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
      reorder();
      applyFilter();
      if (cfg.onData) cfg.onData(list);
      // Первый запуск: инструмент ещё не выбран — выбираем первый из списка.
      if (current == null && firstId != null && cfg.onSelect) cfg.onSelect(firstId);
    }

    /** Подсветить выбранную строку (композитор зовёт при смене инструмента). */
    function setActive(id) {
      Object.entries(rowEls).forEach(([k, el]) => el.classList.toggle('active', +k === id));
    }

    // ---- События -----------------------------------------------------------
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

    rowsEl.addEventListener('click', onRowsClick);
    headEl.addEventListener('click', onHeadClick);
    searchEl.addEventListener('input', onSearch);

    applyCols();
    refresh();
    if (cfg.interval) timer = setInterval(refresh, cfg.interval);

    return {
      refresh,
      setActive,
      /** Текущая видимость колонок (копия — внутреннее состояние не отдаём наружу). */
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
        root.innerHTML = '';
      },
    };
  }

  window.Watchlist = { mount };
})();
