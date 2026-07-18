/* ============================================================================
 * drawings.js — инструменты рисования на графике (оверлей-canvas над Lightweight
 * Charts). Отдельный модуль: подключается ПОСЛЕ app.js, читает window.__lw.
 *
 * Якоря объектов хранятся в координатах данных (logical-индекс бара + цена), поэтому
 * рисунки двигаются/масштабируются вместе с графиком. При перезагрузке данных
 * (смена инструмента/ТФ — событие 'chartReload') объекты очищаются: logical-индексы
 * становятся невалидными после setData.
 * ========================================================================== */
(function () {
  const LW = window.__lw;
  if (!LW) { console.warn('[drawings] window.__lw не найден'); return; }
  const chart = LW.chart;
  const cvs = document.getElementById('drawLayer');
  const bar = document.getElementById('drawTools');
  if (!cvs || !bar) { console.warn('[drawings] нет #drawLayer/#drawTools'); return; }
  const host = cvs.parentElement; // .chartwrap
  const ctx = cvs.getContext('2d');
  const ACCENT = '#f0b90b';

  // --- Определения инструментов: сколько точек нужно, подпись, иконка ---------
  const TOOLS = {
    trend:     { pts: 2, tip: 'Trend line' },
    ray:       { pts: 2, tip: 'Ray' },
    extended:  { pts: 2, tip: 'Extended line' },
    info:      { pts: 2, tip: 'Info line' },
    angle:     { pts: 2, tip: 'Trend angle' },
    hline:     { pts: 1, tip: 'Horizontal line' },
    hray:      { pts: 1, tip: 'Horizontal ray' },
    vline:     { pts: 1, tip: 'Vertical line' },
    cross:     { pts: 1, tip: 'Cross line' },
    channel:   { pts: 3, tip: 'Parallel channel' },
    pitchfork: { pts: 3, tip: 'Pitchfork' },
  };
  // Порядок кнопок в тулбаре (null = разделитель).
  const ORDER = ['cursor', null, 'trend', 'ray', 'extended', 'info', 'angle', null,
    'hline', 'hray', 'vline', 'cross', null, 'channel', 'pitchfork', null, 'undo', 'clear'];

  // Компактные SVG-иконки (16×16, stroke=currentColor).
  const svg = (inner) => `<svg viewBox="0 0 16 16" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" stroke-linejoin="round">${inner}</svg>`;
  const ICON = {
    cursor: svg('<path d="M3 2l9 5-4 1-1 5z"/>'),
    trend: svg('<path d="M2 13L14 3"/><circle cx="2" cy="13" r="1.3"/><circle cx="14" cy="3" r="1.3"/>'),
    ray: svg('<path d="M2 13L14 3"/><circle cx="2" cy="13" r="1.3"/><path d="M11 3h3v3"/>'),
    extended: svg('<path d="M1 14L15 2"/><path d="M1 14l2-1.7M15 2l-2 1.7"/>'),
    info: svg('<path d="M2 13L14 3"/><path d="M8 6.5v.2M8 8.5v2"/>'),
    angle: svg('<path d="M2 13h9"/><path d="M2 13L12 5"/><path d="M6 13a4 4 0 0 1 1.3-3"/>'),
    hline: svg('<path d="M1 8h14"/>'),
    hray: svg('<path d="M4 8h11"/><circle cx="4" cy="8" r="1.3"/>'),
    vline: svg('<path d="M8 1v14"/>'),
    cross: svg('<path d="M1 8h14M8 1v14"/>'),
    channel: svg('<path d="M2 12L14 5"/><path d="M2 15L14 8"/>'),
    pitchfork: svg('<path d="M3 3v10"/><path d="M3 4h10"/><path d="M3 8h10"/><path d="M13 4v9"/><path d="M8 4v11"/>'),
    undo: svg('<path d="M6 4L2 7l4 3"/><path d="M2 7h7a4 4 0 1 1 0 8H6"/>'),
    clear: svg('<path d="M3 5h10M6 5V3h4v2M5 5l1 9h4l1-9"/>'),
  };

  // --- Состояние -------------------------------------------------------------
  let active = null;      // текущий инструмент (null = курсор)
  let pending = [];       // уже поставленные точки текущего объекта
  const drawings = [];    // готовые объекты {type, pts:[{logical,price}]}

  // --- Тулбар ----------------------------------------------------------------
  const btns = {};
  for (const key of ORDER) {
    if (key === null) { const sep = document.createElement('div'); sep.className = 'drawsep'; bar.appendChild(sep); continue; }
    const b = document.createElement('button');
    b.className = 'drawbtn'; b.innerHTML = ICON[key];
    b.dataset.tip = key === 'cursor' ? 'Cursor' : key === 'undo' ? 'Undo' : key === 'clear' ? 'Clear all' : TOOLS[key].tip;
    b.addEventListener('click', () => onToolClick(key));
    bar.appendChild(b); btns[key] = b;
  }
  btns.cursor.classList.add('active');

  function onToolClick(key) {
    if (key === 'undo') { drawings.pop(); pending = []; redraw(); return; }
    if (key === 'clear') { drawings.length = 0; pending = []; setActive(null); redraw(); return; }
    setActive(key === 'cursor' ? null : key);
  }
  function setActive(tool) {
    active = tool; pending = [];
    cvs.style.pointerEvents = tool ? 'auto' : 'none';
    cvs.style.cursor = tool ? 'crosshair' : 'default';
    Object.values(btns).forEach((b) => b.classList.remove('active'));
    (btns[tool] || btns.cursor).classList.add('active');
    redraw();
  }

  // --- Преобразования координат ---------------------------------------------
  const ts = () => chart.timeScale();
  function toPx(a) {
    const x = ts().logicalToCoordinate(a.logical);
    const s = LW.series();
    const y = s ? s.priceToCoordinate(a.price) : null;
    return (x == null || y == null) ? null : { x, y };
  }
  function fromMouse(mx, my) {
    const logical = ts().coordinateToLogical(mx);
    const s = LW.series();
    const price = s ? s.coordinateToPrice(my) : null;
    return (logical == null || price == null) ? null : { logical, price };
  }

  // --- Геометрия рисования ---------------------------------------------------
  const W = () => host.clientWidth, H = () => host.clientHeight;
  const BIG = 6000;
  function stroke(fn, color, width, dash) {
    ctx.save(); ctx.strokeStyle = color || ACCENT; ctx.lineWidth = width || 1.5;
    ctx.setLineDash(dash || []); ctx.beginPath(); fn(); ctx.stroke(); ctx.restore();
  }
  function seg(a, b) { stroke(() => { ctx.moveTo(a.x, a.y); ctx.lineTo(b.x, b.y); }); }
  function dir(a, b) { const dx = b.x - a.x, dy = b.y - a.y, L = Math.hypot(dx, dy) || 1; return { x: dx / L, y: dy / L }; }
  function extend(a, d, len) { return { x: a.x + d.x * len, y: a.y + d.y * len }; }
  function dot(p) { ctx.save(); ctx.fillStyle = ACCENT; ctx.beginPath(); ctx.arc(p.x, p.y, 3, 0, 7); ctx.fill(); ctx.restore(); }
  function label(p, text, dy) {
    ctx.save(); ctx.font = '11px ui-monospace,monospace'; ctx.fillStyle = ACCENT;
    ctx.textBaseline = 'bottom'; ctx.fillText(text, p.x + 6, p.y + (dy || -6)); ctx.restore();
  }

  function render(d) {
    const pts = d.pts.map(toPx);
    if (pts.some((p) => !p)) return;
    const t = d.type, a = pts[0], b = pts[1], c = pts[2];
    const oneShot = t === 'hline' || t === 'hray' || t === 'vline' || t === 'cross';
    if (!oneShot && !b) return; // двух/трёхточечные не рисуем, пока нет второй точки
    switch (t) {
      case 'trend': seg(a, b); dot(a); dot(b); break;
      case 'ray': { const dd = dir(a, b); seg(a, extend(a, dd, BIG)); dot(a); dot(b); break; }
      case 'extended': { const dd = dir(a, b); seg(extend(a, dd, -BIG), extend(a, dd, BIG)); dot(a); dot(b); break; }
      case 'hline': stroke(() => { ctx.moveTo(0, a.y); ctx.lineTo(W(), a.y); }); dot(a); break;
      case 'hray': stroke(() => { ctx.moveTo(a.x, a.y); ctx.lineTo(W(), a.y); }); dot(a); break;
      case 'vline': stroke(() => { ctx.moveTo(a.x, 0); ctx.lineTo(a.x, H()); }); dot(a); break;
      case 'cross': stroke(() => { ctx.moveTo(0, a.y); ctx.lineTo(W(), a.y); ctx.moveTo(a.x, 0); ctx.lineTo(a.x, H()); }); dot(a); break;
      case 'info': {
        seg(a, b); dot(a); dot(b);
        const dP = d.pts[1].price - d.pts[0].price;
        const pct = d.pts[0].price ? dP / d.pts[0].price * 100 : 0;
        const bars = Math.round(d.pts[1].logical - d.pts[0].logical);
        label(b, `${dP >= 0 ? '+' : ''}${dP.toPrecision(5)} (${pct >= 0 ? '+' : ''}${pct.toFixed(2)}%) ${bars} bars`);
        break;
      }
      case 'angle': {
        seg(a, b); dot(a); dot(b);
        const deg = Math.atan2(-(b.y - a.y), b.x - a.x) * 180 / Math.PI;
        stroke(() => ctx.arc(a.x, a.y, 26, 0, -Math.atan2(-(b.y - a.y), b.x - a.x), b.y < a.y), ACCENT, 1, [3, 3]);
        label(a, `${deg.toFixed(1)}°`, -10);
        break;
      }
      case 'channel': {
        if (!c) { seg(a, b); dot(a); dot(b); break; }
        const dx = b.x - a.x, dy = b.y - a.y, L2 = dx * dx + dy * dy || 1;
        const tProj = ((c.x - a.x) * dx + (c.y - a.y) * dy) / L2;
        const foot = { x: a.x + dx * tProj, y: a.y + dy * tProj };
        const off = { x: c.x - foot.x, y: c.y - foot.y };
        const a2 = { x: a.x + off.x, y: a.y + off.y }, b2 = { x: b.x + off.x, y: b.y + off.y };
        ctx.save(); ctx.fillStyle = 'rgba(240,185,11,.07)'; ctx.beginPath();
        ctx.moveTo(a.x, a.y); ctx.lineTo(b.x, b.y); ctx.lineTo(b2.x, b2.y); ctx.lineTo(a2.x, a2.y); ctx.closePath(); ctx.fill(); ctx.restore();
        seg(a, b); seg(a2, b2); dot(a); dot(b); dot(c);
        break;
      }
      case 'pitchfork': {
        if (!c) { seg(a, b); dot(a); dot(b); break; }
        const mid = { x: (b.x + c.x) / 2, y: (b.y + c.y) / 2 };
        const dm = dir(a, mid);
        seg(a, mid); seg(mid, extend(mid, dm, BIG));       // медиана
        seg(b, extend(b, dm, BIG));                        // верхняя вилка
        seg(c, extend(c, dm, BIG));                        // нижняя вилка
        stroke(() => { ctx.moveTo(b.x, b.y); ctx.lineTo(c.x, c.y); }, ACCENT, 1, [4, 3]);
        dot(a); dot(b); dot(c);
        break;
      }
      default: break;
    }
  }

  // --- Отрисовка всего слоя --------------------------------------------------
  function resize() {
    const w = W(), h = H(), dpr = window.devicePixelRatio || 1;
    if (cvs.width !== Math.round(w * dpr) || cvs.height !== Math.round(h * dpr)) {
      cvs.width = Math.round(w * dpr); cvs.height = Math.round(h * dpr);
      cvs.style.width = w + 'px'; cvs.style.height = h + 'px';
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
    }
  }
  function redraw(previewPt) {
    resize();
    ctx.clearRect(0, 0, W(), H());
    for (const d of drawings) render(d);
    if (active && pending.length && previewPt) render({ type: active, pts: [...pending, previewPt] });
    else if (active && pending.length) render({ type: active, pts: pending });
  }

  // --- Ввод мышью ------------------------------------------------------------
  cvs.addEventListener('mousedown', (e) => {
    if (!active) return;
    const a = fromMouse(e.offsetX, e.offsetY); if (!a) return;
    pending.push(a);
    if (pending.length >= TOOLS[active].pts) { drawings.push({ type: active, pts: pending }); setActive(null); }
    else redraw();
  });
  cvs.addEventListener('mousemove', (e) => {
    if (!active || !pending.length) return;
    redraw(fromMouse(e.offsetX, e.offsetY));
  });
  // ПКМ / Esc — отмена текущего объекта или возврат к курсору.
  cvs.addEventListener('contextmenu', (e) => { if (active) { e.preventDefault(); setActive(null); } });
  window.addEventListener('keydown', (e) => { if (e.key === 'Escape' && active) setActive(null); });

  // --- Перерисовка при изменениях графика ------------------------------------
  ts().subscribeVisibleLogicalRangeChange(() => redraw());
  new ResizeObserver(() => redraw()).observe(host);
  window.addEventListener('chartReload', () => { drawings.length = 0; pending = []; redraw(); });

  redraw();
})();
