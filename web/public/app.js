'use strict';

// ============================ State ========================================
let META = {}, selected = null, tf = 3600, orderMode = 'market';
let chartType = 'candles', priceType = 'mid', showPips = false, showSLTP = true, sizeUnit = 'lot';
const overlays = { sma: 0, ema: 0, wma: 0, supertrend: 0, psar: 0, ichimoku: 0, alligator: 0, boll: 0, keltner: 0, donchian: 0, vwap: 0 };
let oscillator = '';
let rawCandles = [], currentDeals = [];
const rowEls = {};
let sortKey = null, sortDir = -1;
const favs = new Set(JSON.parse(localStorage.getItem('favs') || '[]'));

const token = () => document.getElementById('userSel').value;
const pdec = (id) => (META[id]?.price_decimals ?? 2);
const qdec = (id) => (META[id]?.qty_decimals ?? 3);
const pscale = (id) => 10 ** pdec(id);
const qscale = (id) => 10 ** qdec(id);
const fmtP = (id, raw) => (raw / pscale(id)).toLocaleString('en-US', { minimumFractionDigits: pdec(id), maximumFractionDigits: pdec(id) });
const fmtQ = (id, raw) => (raw / qscale(id)).toFixed(qdec(id));
const usd = (c) => '$' + (c / 100).toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
const TFN = { 60: '1m', 300: '5m', 900: '15m', 1800: '30m', 3600: '1h', 14400: '4h', 86400: '1d', 604800: '1w' };
const settings = Object.assign(
  { shadows: true, shadowCustom: false, shadowColor: '#787b86', hollowColor: '#26a69a', priceLineWidth: 1, addLine: false, addLineWidth: 1, minChange: 'default' },
  JSON.parse(localStorage.getItem('settings') || '{}')
);
const saveSettings = () => localStorage.setItem('settings', JSON.stringify(settings));

// ---- Icons (inline SVG, currentColor) -------------------------------------
const ICONS = {
  candles: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3"><path d="M5 2v3M5 11v3M11 3v3M11 10v3"/><rect x="3.3" y="5" width="3.4" height="6" fill="currentColor" stroke="none"/><rect x="9.3" y="6" width="3.4" height="4" fill="currentColor" stroke="none"/></svg>',
  hollow: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3"><path d="M5 2v3M5 11v3M11 3v3M11 10v3"/><rect x="3.3" y="5" width="3.4" height="6"/><rect x="9.3" y="6" width="3.4" height="4"/></svg>',
  bars: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3"><path d="M5 2v12M2.5 5H5M5 9h2.5M11 3v10M8.5 6H11M11 10h2.5"/></svg>',
  line: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><polyline points="2 12 6 7 9 10 14 3"/></svg>',
  area: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3"><path d="M2 12 6 7 9 10 14 3V14H2Z" fill="currentColor" fill-opacity=".3" stroke="none"/><polyline points="2 12 6 7 9 10 14 3" stroke-width="1.5"/></svg>',
  indicators: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><path d="M2 10c2-4 4-4 6 0s4 4 6-1"/><path d="M2 6c2-3 4 3 6-1s4-2 6 1" opacity=".5"/></svg>',
  wallet: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3"><rect x="2" y="4" width="11.5" height="8.5" rx="1.6"/><path d="M9.6 7.6H13v2.4H9.6a1.2 1.2 0 0 1 0-2.4Z" fill="currentColor" stroke="none"/></svg>',
  star: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.2"><path d="M8 2l1.8 3.9 4.2.4-3.2 2.9.9 4.1L8 11.3 4.3 13.2l.9-4.1L2 6.3l4.2-.4z" stroke-linejoin="round"/></svg>',
  caret: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6"><path d="M4 6l4 4 4-4"/></svg>',
  reset: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><path d="M12.5 8a4.5 4.5 0 1 1-1.3-3.2"/><path d="M12.5 2.5V5H10"/></svg>',
  fullscreen: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><path d="M2 6V2h4M14 6V2h-4M2 10v4h4M14 10v4h-4"/></svg>',
  settings: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.25"><circle cx="8" cy="8" r="2.1"/><path d="M8 1.6v1.8M8 12.6v1.8M1.6 8h1.8M12.6 8h1.8M3.5 3.5l1.3 1.3M11.2 11.2l1.3 1.3M12.5 3.5l-1.3 1.3M4.8 11.2l-1.3 1.3"/></svg>',
};
function renderIcons(root = document) { root.querySelectorAll('[data-icon]').forEach((el) => { const n = el.dataset.icon; if (ICONS[n]) el.innerHTML = ICONS[n]; }); }
function hue(s) { let h = 0; for (let i = 0; i < s.length; i++) h = (h * 31 + s.charCodeAt(i)) % 360; return h; }
function coinHtml(b) { return `<span class="coinwrap"><span class="coinbadge" style="background:hsl(${hue(b)} 52% 40%)">${b[0]}</span><img class="coin" src="/vendor/coins/${b.toLowerCase()}.svg" onerror="this.remove()" alt=""></span>`; }

// ---- Legend (chart info card) ---------------------------------------------
function fmtDT(t) { return new Date(t * 1000).toLocaleString('en-GB', { timeZone: 'UTC', day: '2-digit', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit', hour12: false }) + ' UTC'; }
function updateLegend(id, t, c) {
  const rf = (x) => x.toLocaleString('en-US', { minimumFractionDigits: pdec(id), maximumFractionDigits: pdec(id) });
  document.getElementById('lgSym').textContent = META[id]?.symbol || '';
  document.getElementById('lgTf').textContent = TFN[tf] || '';
  document.getElementById('lgTime').textContent = fmtDT(t);
  document.getElementById('lgOhlc').innerHTML = `O <b>${rf(c.open)}</b>  H <b>${rf(c.high)}</b>  L <b>${rf(c.low)}</b>  C <b class="${c.close >= c.open ? 'up' : 'down'}">${rf(c.close)}</b>`;
}
function refreshLegend() { if (selected == null) return; let c; if (formingRaw) { const s = pscale(selected); c = { time: formingRaw.time, open: formingRaw.open / s, high: formingRaw.high / s, low: formingRaw.low / s, close: formingRaw.close / s }; } else if (rawCandles.length) c = rawCandles[rawCandles.length - 1]; if (c) updateLegend(selected, c.time, c); }

// ---- Favorites / sorting --------------------------------------------------
function toggleFav(id) { if (favs.has(id)) favs.delete(id); else favs.add(id); localStorage.setItem('favs', JSON.stringify([...favs])); rowEls[id]?.querySelector('.star')?.classList.toggle('on', favs.has(id)); reorderRows(); }
const SORT_METRIC = { change: (it) => it.change, sell: (it) => it.bid ?? -Infinity, buy: (it) => it.ask ?? -Infinity, spread: (it) => (it.ask != null && it.bid != null ? it.ask - it.bid : -Infinity), high: (it) => it.high ?? -Infinity };
function reorderRows() {
  const cont = document.getElementById('instruments');
  Object.keys(rowEls).map(Number).sort((a, b) => {
    const fa = favs.has(a), fb = favs.has(b); if (fa !== fb) return fb - fa;
    if (sortKey && META[a] && META[b]) { const m = SORT_METRIC[sortKey]; const d = (m(META[a]) - m(META[b])) * sortDir; if (d) return d; }
    return a - b;
  }).forEach((id) => cont.appendChild(rowEls[id]));
}

// ===== Indicators — вынесены в модуль indicators.js (ADR-015), подключается до app.js =====
// toSeries — чартовый глюкод (значения → {time,value} для Lightweight Charts), остаётся здесь.
const { HL2, HLC3, sma, ema, wma, smma, stdev, trueRange, atr, rsi, highest, lowest, stoch, cci, momentum, roc, williams, ao, acOsc, obv, adLine, cmf, mfi, forceIndex, macdFull, adxDI, bollinger, keltner, donchian, superTrend, psar, ichimoku, vwap, alligator, heikin } = window.Indicators;
const toSeries = (arr, t) => arr.map((v, i) => (v == null || !isFinite(v) ? null : { time: t[i], value: v })).filter(Boolean);


const OVERLAYS = {
  sma: (cs) => [{ color: '#4fc3f7', values: sma(cs.map((c) => c.close), 20) }],
  ema: (cs) => [{ color: '#ba68c8', values: ema(cs.map((c) => c.close), 20) }],
  wma: (cs) => [{ color: '#ffb74d', values: wma(cs.map((c) => c.close), 20) }],
  supertrend: superTrend, psar, ichimoku, alligator, boll: bollinger, keltner, donchian, vwap,
};
const OSC = {
  rsi: (cs) => [{ color: '#f0b90b', values: rsi(cs.map((c) => c.close), 14) }],
  stoch: (cs) => { const [k, d] = stoch(cs, 14, 3); return [{ color: '#f0b90b', values: k }, { color: '#ef5350', values: d }]; },
  cci: (cs) => [{ color: '#f0b90b', values: cci(cs, 20) }],
  momentum: (cs) => [{ color: '#f0b90b', values: momentum(cs.map((c) => c.close), 10) }],
  williams: (cs) => [{ color: '#f0b90b', values: williams(cs, 14) }],
  roc: (cs) => [{ color: '#f0b90b', values: roc(cs.map((c) => c.close), 12) }],
  adx: (cs) => { const [a, p, n] = adxDI(cs, 14); return [{ color: '#f0b90b', values: a }, { color: '#26a69a', values: p }, { color: '#ef5350', values: n }]; },
  macd: (cs) => { const [l, s] = macdFull(cs.map((c) => c.close)); return [{ color: '#42a5f5', values: l }, { color: '#ef5350', values: s }]; },
  atr: (cs) => [{ color: '#f0b90b', values: atr(cs, 14) }],
  stddev: (cs) => [{ color: '#f0b90b', values: stdev(cs.map((c) => c.close), 20) }],
  obv: (cs) => [{ color: '#f0b90b', values: obv(cs) }],
  ad: (cs) => [{ color: '#f0b90b', values: adLine(cs) }],
  cmf: (cs) => [{ color: '#f0b90b', values: cmf(cs, 20) }],
  mfi: (cs) => [{ color: '#f0b90b', values: mfi(cs, 14) }],
  force: (cs) => [{ color: '#f0b90b', values: forceIndex(cs, 13) }],
  ao: (cs) => [{ color: '#f0b90b', values: ao(cs) }],
  ac: (cs) => [{ color: '#f0b90b', values: acOsc(cs) }],
  gator: (cs) => { const [j, t, l] = alligator(cs).map((x) => x.values); return [{ color: '#26a69a', values: j.map((v, i) => (v != null && t[i] != null ? Math.abs(v - t[i]) : null)) }, { color: '#ef5350', values: t.map((v, i) => (v != null && l[i] != null ? -Math.abs(v - l[i]) : null)) }]; },
};

// ============================ Chart ========================================
const chartEl = document.getElementById('chart');
const chart = LightweightCharts.createChart(chartEl, {
  layout: { background: { color: '#0b0e14' }, textColor: '#6b7688', attributionLogo: false },
  grid: { vertLines: { color: 'rgba(31,39,53,.4)' }, horzLines: { color: 'rgba(31,39,53,.4)' } },
  rightPriceScale: { borderColor: '#1f2735' },
  timeScale: { borderColor: '#1f2735', timeVisible: true, secondsVisible: false },
  crosshair: { mode: LightweightCharts.CrosshairMode.Normal },
});
const volume = chart.addHistogramSeries({ priceFormat: { type: 'volume' }, priceScaleId: '', color: '#2b3648' });
volume.priceScale().applyOptions({ scaleMargins: { top: 0.85, bottom: 0 } });
// Хуки для модуля рисования (drawings.js подключается после app.js).
window.__lw = { chart, el: chartEl, series: () => mainSeries };

let mainSeries = null, overlaySeries = [], oscSeriesList = [], priceLines = [];
let lastBarTime = 0, formingRaw = null;
const volColor = (c) => (c.close >= c.open ? 'rgba(38,166,154,.4)' : 'rgba(239,83,80,.4)');
const isLineType = () => (chartType === 'line' || chartType === 'area');

function candleColors() {
  const wick = (def) => (settings.shadows ? (settings.shadowCustom ? settings.shadowColor : def) : 'rgba(0,0,0,0)');
  if (chartType === 'hollow') return { upColor: 'rgba(0,0,0,0)', downColor: 'rgba(0,0,0,0)', borderUpColor: settings.hollowColor, borderDownColor: '#ef5350', wickUpColor: wick(settings.hollowColor), wickDownColor: wick('#ef5350') };
  return { upColor: '#26a69a', downColor: '#ef5350', borderUpColor: '#26a69a', borderDownColor: '#ef5350', wickUpColor: wick('#26a69a'), wickDownColor: wick('#ef5350') };
}
function makeMainSeries() {
  if (mainSeries) chart.removeSeries(mainSeries);
  if (chartType === 'candles' || chartType === 'hollow' || chartType === 'heikin') mainSeries = chart.addCandlestickSeries(candleColors());
  else if (chartType === 'bars') mainSeries = chart.addBarSeries({ upColor: '#26a69a', downColor: '#ef5350' });
  else if (chartType === 'line') mainSeries = chart.addLineSeries({ color: '#f0b90b', lineWidth: 2 });
  else mainSeries = chart.addAreaSeries({ lineColor: '#f0b90b', topColor: 'rgba(240,185,11,.25)', bottomColor: 'rgba(240,185,11,.02)' });
  applySettings();
}
function applySettings() {
  if (!mainSeries || selected == null) return;
  let precision = pdec(selected), minMove = 1 / pscale(selected);
  if (settings.minChange === '1:1') { precision = 0; minMove = 1; }
  else if (settings.minChange === '1:10') { precision = pdec(selected) + 1; minMove = 1 / (pscale(selected) * 10); }
  const opts = { priceFormat: { type: 'price', precision, minMove }, priceLineVisible: settings.priceLineWidth > 0, priceLineWidth: Math.max(1, settings.priceLineWidth || 1) };
  if (chartType === 'candles' || chartType === 'hollow' || chartType === 'heikin') Object.assign(opts, candleColors());
  mainSeries.applyOptions(opts);
  redrawPriceLines();
}
function mainData() {
  if (isLineType()) return rawCandles.map((c) => ({ time: c.time, value: c.close }));
  const src = chartType === 'heikin' ? heikin(rawCandles) : rawCandles;
  return src.map((c) => ({ time: c.time, open: c.open, high: c.high, low: c.low, close: c.close }));
}
function applyIndicators() {
  overlaySeries.forEach((s) => chart.removeSeries(s)); overlaySeries = [];
  oscSeriesList.forEach((s) => chart.removeSeries(s)); oscSeriesList = [];
  const times = rawCandles.map((c) => c.time);
  for (const key in overlays) {
    if (!overlays[key] || !OVERLAYS[key]) continue;
    for (const ln of OVERLAYS[key](rawCandles)) { const s = chart.addLineSeries({ color: ln.color, lineWidth: ln.width || 1, priceLineVisible: false, lastValueVisible: false }); s.setData(toSeries(ln.values, times)); overlaySeries.push(s); }
  }
  if (oscillator && OSC[oscillator]) {
    volume.applyOptions({ visible: false });
    OSC[oscillator](rawCandles).forEach((ln, i) => { const s = chart.addLineSeries({ color: ln.color, lineWidth: 1, priceScaleId: 'osc', priceLineVisible: false, lastValueVisible: false }); if (i === 0) s.priceScale().applyOptions({ scaleMargins: { top: 0.78, bottom: 0 } }); s.setData(toSeries(ln.values, times)); oscSeriesList.push(s); });
  } else volume.applyOptions({ visible: true });
}
function redrawPriceLines() {
  if (!mainSeries) return;
  priceLines.forEach((l) => mainSeries.removePriceLine(l)); priceLines = [];
  const s = pscale(selected);
  if (showSLTP) for (const d of currentDeals) {
    priceLines.push(mainSeries.createPriceLine({ price: d.entry / s, color: '#8899aa', lineStyle: 2, lineWidth: 1, title: d.side === 'buy' ? 'L' : 'S' }));
    if (d.sl != null) priceLines.push(mainSeries.createPriceLine({ price: d.sl / s, color: '#ef5350', lineWidth: 1, title: 'SL' }));
    if (d.tp != null) priceLines.push(mainSeries.createPriceLine({ price: d.tp / s, color: '#26a69a', lineWidth: 1, title: 'TP' }));
  }
  if (settings.addLine && rawCandles.length > 1) priceLines.push(mainSeries.createPriceLine({ price: rawCandles[rawCandles.length - 2].close, color: '#f0b90b', lineStyle: 3, lineWidth: Math.max(1, settings.addLineWidth || 1), title: 'ref' }));
}

new ResizeObserver(() => chart.applyOptions({ width: chartEl.clientWidth, height: chartEl.clientHeight })).observe(chartEl);
chart.subscribeCrosshairMove((p) => { const id = selected; if (id == null) return; const d = p.seriesData?.get(mainSeries); if (p.time && d) { const c = d.close != null ? d : { open: d.value, high: d.value, low: d.value, close: d.value }; updateLegend(id, p.time, c); } else refreshLegend(); });

async function loadCandles(id, timeframe) {
  const load = document.getElementById('loading'); load.classList.remove('hidden');
  try {
    const r = await fetch(`/candles/${id}?tf=${timeframe}&limit=300`);
    const raw = await r.json().catch(() => []);
    const s = pscale(id), v = qscale(id);
    rawCandles = raw.map((c) => ({ time: c.time, open: c.open / s, high: c.high / s, low: c.low / s, close: c.close / s, volume: c.volume / v }));
    makeMainSeries();
    mainSeries.setData(mainData());
    volume.setData(rawCandles.map((c) => ({ time: c.time, value: c.volume, color: volColor(c) })));
    applyIndicators(); redrawPriceLines(); computeSignals();
    if (rawCandles.length) { const last = raw[raw.length - 1]; lastBarTime = last.time; formingRaw = { ...last }; const n = rawCandles.length; chart.timeScale().setVisibleLogicalRange({ from: Math.max(0, n - 90), to: n + 3 }); } else { lastBarTime = 0; formingRaw = null; }
    refreshLegend();
    window.dispatchEvent(new Event('chartReload')); // модуль рисования очищает объекты при смене данных
  } finally { load.classList.add('hidden'); }
}
function drawForming() { if (!formingRaw || !mainSeries || chartType === 'heikin') return; const s = pscale(selected); mainSeries.update(isLineType() ? { time: formingRaw.time, value: formingRaw.close / s } : { time: formingRaw.time, open: formingRaw.open / s, high: formingRaw.high / s, low: formingRaw.low / s, close: formingRaw.close / s }); refreshLegend(); }
function onLiveTrade(id, price, qty) { if (id !== selected) return; const now = Math.floor(Date.now() / 1000); const b = now - (now % tf); if (!formingRaw || b > formingRaw.time) formingRaw = { time: b, open: price, high: price, low: price, close: price, volume: qty }; else { formingRaw.high = Math.max(formingRaw.high, price); formingRaw.low = Math.min(formingRaw.low, price); formingRaw.close = price; } if (formingRaw.time >= lastBarTime) { lastBarTime = formingRaw.time; drawForming(); } }
async function syncLast() { const id = selected; if (id == null) return; const r = await fetch(`/candles/${id}?tf=${tf}&limit=2`); const raw = await r.json().catch(() => []); if (!raw.length) return; const last = raw[raw.length - 1]; if (last.time >= lastBarTime) { lastBarTime = last.time; formingRaw = { ...last }; drawForming(); } }

// ============================ Instruments ==================================
async function refreshInstruments() {
  const r = await fetch('/instruments'); const list = await r.json().catch(() => []);
  const cont = document.getElementById('instruments');
  for (const it of list) {
    META[it.id] = it; let el = rowEls[it.id]; const isNew = !el;
    if (isNew) { el = document.createElement('div'); el.className = 'irow'; el.dataset.id = it.id; cont.appendChild(el); rowEls[it.id] = el; }
    const up = it.change >= 0; const [b, q] = it.symbol.split('-');
    el.innerHTML = `<span class="star ${favs.has(it.id) ? 'on' : ''}" data-icon="star"></span>` +
      `<div class="sym">${coinHtml(b)}<span class="symtxt">${b}<small>/${q || 'USDT'}</small></span></div>` +
      `<div class="num chg c-change ${up ? 'up' : 'down'}">${up ? '+' : ''}${it.change.toFixed(2)}%</div>` +
      `<div class="num sell c-sell">${it.bid != null ? fmtP(it.id, it.bid) : '—'}</div>` +
      `<div class="num buy c-buy">${it.ask != null ? fmtP(it.id, it.ask) : '—'}</div>` +
      `<div class="num muted c-spread">${it.bid != null && it.ask != null ? it.ask - it.bid : '—'}</div>` +
      `<div class="num c-high">${it.high != null ? fmtP(it.id, it.high) : '—'}</div>`;
    el.classList.toggle('active', it.id === selected);
    if (isNew && selected === null) selectInstrument(it.id);
  }
  renderIcons(cont); reorderRows(); updateDealPanel();
}
function refPrice(it) { if (!it || it.bid == null || it.ask == null) return it?.last; if (priceType === 'ask') return it.ask; if (priceType === 'bid') return it.bid; return Math.round((it.bid + it.ask) / 2); }
function updateDealPanel() {
  const it = META[selected]; if (!it) return;
  document.getElementById('dealSym').textContent = it.symbol;
  const ref = refPrice(it);
  document.getElementById('dealSub').textContent = `${priceType.toUpperCase()} ${ref != null ? fmtP(it.id, ref) : '—'}`;
  const chg = document.getElementById('dealChange'); const up = it.change >= 0; chg.textContent = `${up ? '+' : ''}${it.change.toFixed(2)}%`; chg.className = `deal__change ${up ? 'up' : 'down'}`;
  document.getElementById('askPx').textContent = it.ask != null ? fmtP(it.id, it.ask) : '—';
  document.getElementById('bidPx').textContent = it.bid != null ? fmtP(it.id, it.bid) : '—';
  document.getElementById('watermark').textContent = it.symbol;
  const pips = document.getElementById('pipsInfo');
  if (showPips && it.bid != null && it.ask != null) { pips.classList.remove('hidden'); pips.textContent = `Spread: ${it.ask - it.bid} pips`; } else pips.classList.add('hidden');
  if (orderMode === 'limit' && !document.getElementById('price').value && ref != null) document.getElementById('price').value = (ref / pscale(it.id)).toFixed(pdec(it.id));
  updateSizeEq();
}

// ---- Order book + asset stats ---------------------------------------------
// Стакан и статистика — переиспользуемые виджеты (ADR-015), паттерн mount(root, ctx).
const book = OrderBook.mount(document.getElementById('book'), {
  instrument: () => selected,
  fetch: (id) => fetch(`/depth/${id}`).then((r) => r.json()),
  onPick: (price) => { setMode('limit'); const inp = document.getElementById('price'); inp.value = price; inp.classList.add('flash'); setTimeout(() => inp.classList.remove('flash'), 400); },
  interval: 700,
});
const stats = AssetStats.mount(document.getElementById('statbar'), {
  instrument: () => selected,
  fetch: (id) => fetch(`/stats/${id}`).then((r) => r.json()),
  labels: TFN,
  interval: 5000,
});
async function selectInstrument(id) { selected = id; Object.entries(rowEls).forEach(([k, el]) => el.classList.toggle('active', +k === id)); document.getElementById('price').value = ''; document.getElementById('sl').value = ''; document.getElementById('tp').value = ''; await loadCandles(id, tf); updateDealPanel(); pollAccount(); pollDeals(); book.refresh(); stats.refresh(); }
document.getElementById('tfs').addEventListener('click', (e) => { const btn = e.target.closest('button'); if (!btn) return; tf = +btn.dataset.tf; document.querySelectorAll('#tfs button').forEach((b) => b.classList.toggle('active', b === btn)); if (selected != null) loadCandles(selected, tf); });

// ============================ Toolbar (dropdowns) ==========================
function closeDD() { document.querySelectorAll('.dd').forEach((d) => d.classList.add('hidden')); }
function toggleDD(id) { const m = document.getElementById(id); const wasHidden = m.classList.contains('hidden'); closeDD(); if (wasHidden) m.classList.remove('hidden'); }
document.addEventListener('click', (e) => { if (!e.target.closest('.tb-dd')) closeDD(); });

document.getElementById('ctBtn').addEventListener('click', () => toggleDD('ctMenu'));
document.getElementById('ctMenu').addEventListener('click', (e) => { const it = e.target.closest('[data-ct]'); if (!it) return; chartType = it.dataset.ct; document.querySelectorAll('#ctMenu .dd-item').forEach((x) => x.classList.toggle('active', x === it)); const icName = it.querySelector('[data-icon]')?.dataset.icon || 'candles'; const btnIc = document.querySelector('#ctBtn .ic'); if (btnIc) { btnIc.dataset.icon = icName; btnIc.innerHTML = ICONS[icName]; } document.getElementById('ctLabel').textContent = it.textContent.trim(); closeDD(); makeMainSeries(); mainSeries.setData(mainData()); applyIndicators(); redrawPriceLines(); });

document.getElementById('indBtn').addEventListener('click', () => toggleDD('indMenu'));
document.getElementById('indMenu').addEventListener('click', (e) => {
  const ov = e.target.closest('[data-ov]');
  const os = e.target.closest('[data-osc]');
  if (ov) { const k = ov.dataset.ov; overlays[k] = overlays[k] ? 0 : 1; ov.classList.toggle('on', !!overlays[k]); applyIndicators(); }
  else if (os) { oscillator = os.dataset.osc; document.querySelectorAll('#indMenu .osc').forEach((x) => x.classList.toggle('on', x === os)); applyIndicators(); }
});

document.getElementById('ptBtn').addEventListener('click', () => toggleDD('ptMenu'));
document.getElementById('ptMenu').addEventListener('click', (e) => { const it = e.target.closest('[data-pt]'); if (!it) return; priceType = it.dataset.pt; document.querySelectorAll('#ptMenu .dd-item').forEach((x) => x.classList.toggle('active', x === it)); document.getElementById('ptLabel').textContent = it.textContent.trim().toUpperCase(); closeDD(); updateDealPanel(); });

document.getElementById('sltpBtn').addEventListener('click', () => { showSLTP = !showSLTP; document.getElementById('sltpBtn').classList.toggle('on', showSLTP); redrawPriceLines(); });
document.getElementById('pipsBtn').addEventListener('click', () => { showPips = !showPips; document.getElementById('pipsBtn').classList.toggle('on', showPips); updateDealPanel(); pollDeals(); });

// Reset (full) / Fullscreen / Settings
function resetChart() {
  chartType = 'candles';
  Object.keys(overlays).forEach((k) => (overlays[k] = 0));
  oscillator = '';
  document.querySelectorAll('#indMenu [data-ov]').forEach((x) => x.classList.remove('on'));
  document.querySelectorAll('#indMenu .osc').forEach((x) => x.classList.toggle('on', x.dataset.osc === ''));
  document.querySelectorAll('#ctMenu .dd-item').forEach((x) => x.classList.toggle('active', x.dataset.ct === 'candles'));
  const btnIc = document.querySelector('#ctBtn .ic'); if (btnIc) { btnIc.dataset.icon = 'candles'; btnIc.innerHTML = ICONS.candles; }
  document.getElementById('ctLabel').textContent = 'Candles';
  makeMainSeries(); mainSeries.setData(mainData()); applyIndicators(); redrawPriceLines();
  const n = rawCandles.length; if (n) chart.timeScale().setVisibleLogicalRange({ from: Math.max(0, n - 90), to: n + 3 });
}
document.getElementById('resetBtn').addEventListener('click', resetChart);
document.getElementById('fullBtn').addEventListener('click', () => { const cw = document.querySelector('.chartwrap'); if (document.fullscreenElement) document.exitFullscreen(); else cw.requestFullscreen?.(); });
document.getElementById('setBtn').addEventListener('click', () => toggleDD('setMenu'));
function bindSetting(id, key, kind) {
  const el = document.getElementById(id); if (!el) return;
  if (kind === 'bool') el.checked = settings[key]; else el.value = settings[key];
  el.addEventListener('change', () => { settings[key] = kind === 'bool' ? el.checked : (kind === 'num' ? Number(el.value) : el.value); saveSettings(); applySettings(); });
}
bindSetting('sHollow', 'hollowColor', 'str'); bindSetting('sShadows', 'shadows', 'bool'); bindSetting('sShadowCustom', 'shadowCustom', 'bool'); bindSetting('sShadowColor', 'shadowColor', 'str');
bindSetting('sPlw', 'priceLineWidth', 'num'); bindSetting('sAddLine', 'addLine', 'bool'); bindSetting('sAlw', 'addLineWidth', 'num'); bindSetting('sMinChange', 'minChange', 'str');

// Watchlist column visibility
const cols = Object.assign({ change: 1, sell: 1, buy: 1, spread: 1, high: 1 }, JSON.parse(localStorage.getItem('cols') || '{}'));
const COLW = { change: '.82fr', sell: '.85fr', buy: '.85fr', spread: '.7fr', high: '.85fr' };
function applyCols() {
  const order = ['change', 'sell', 'buy', 'spread', 'high'];
  const tracks = ['18px', '1.55fr', ...order.filter((k) => cols[k]).map((k) => COLW[k])];
  const watch = document.querySelector('.watch');
  watch.style.setProperty('--watch-cols', tracks.join(' '));
  order.forEach((k) => watch.classList.toggle('hide-' + k, !cols[k]));
}
function bindCol(id, key) { const el = document.getElementById(id); if (!el) return; el.checked = !!cols[key]; el.addEventListener('change', () => { cols[key] = el.checked ? 1 : 0; localStorage.setItem('cols', JSON.stringify(cols)); applyCols(); }); }
bindCol('cChange', 'change'); bindCol('cSell', 'sell'); bindCol('cBuy', 'buy'); bindCol('cSpread', 'spread'); bindCol('cHigh', 'high');

// Panel/toolbar resizing
const layout = Object.assign({ leftW: 280, rightW: 300, bottomH: 190, toolbarH: 42 }, JSON.parse(localStorage.getItem('layout') || '{}'));
function applyLayout() {
  document.querySelector('.grid').style.gridTemplateColumns = `${layout.leftW}px 6px 1fr 6px ${layout.rightW}px`;
  document.querySelector('.bottom').style.height = layout.bottomH + 'px';
  document.querySelector('.toolbar').style.height = layout.toolbarH + 'px';
}
function initResizers() {
  document.querySelectorAll('[data-rsz]').forEach((h) => h.addEventListener('mousedown', (e) => {
    e.preventDefault(); h.classList.add('drag');
    const type = h.dataset.rsz, sx = e.clientX, sy = e.clientY, sl = layout.leftW, sr = layout.rightW, sb = layout.bottomH, stH = layout.toolbarH;
    const mv = (ev) => {
      if (type === 'left') layout.leftW = Math.min(520, Math.max(180, sl + (ev.clientX - sx)));
      else if (type === 'right') layout.rightW = Math.min(560, Math.max(220, sr - (ev.clientX - sx)));
      else if (type === 'bottom') layout.bottomH = Math.min(window.innerHeight - 220, Math.max(80, sb - (ev.clientY - sy)));
      else layout.toolbarH = Math.min(140, Math.max(40, stH + (ev.clientY - sy)));
      applyLayout();
    };
    const up = () => { document.removeEventListener('mousemove', mv); document.removeEventListener('mouseup', up); document.body.style.userSelect = ''; h.classList.remove('drag'); localStorage.setItem('layout', JSON.stringify(layout)); };
    document.body.style.userSelect = 'none';
    document.addEventListener('mousemove', mv); document.addEventListener('mouseup', up);
  }));
}

// ============================ Tooltip ======================================
const tip = document.getElementById('tip');
document.addEventListener('mouseover', (e) => { const t = e.target.closest('[data-tip]'); if (!t) return; tip.textContent = t.dataset.tip; tip.classList.remove('hidden'); const r = t.getBoundingClientRect(); tip.style.left = r.left + r.width / 2 + 'px'; tip.style.top = r.bottom + 7 + 'px'; });
document.addEventListener('mouseout', (e) => { if (e.target.closest('[data-tip]')) tip.classList.add('hidden'); });

// ============================ WebSocket ====================================
function connectWS() { const proto = location.protocol === 'https:' ? 'wss' : 'ws'; const ws = new WebSocket(`${proto}://${location.host}/stream`); const st = document.getElementById('status'); ws.onopen = () => { st.className = 'status status--on'; }; ws.onclose = () => { st.className = 'status status--off'; setTimeout(connectWS, 1500); }; ws.onmessage = (ev) => { let e; try { e = JSON.parse(ev.data); } catch { return; } for (const x of e) if (x.type === 'trade') onLiveTrade(x.instrument, x.price, x.qty); }; }

// ============================ Order form ===================================
document.getElementById('tabDeal').addEventListener('click', () => setMode('market'));
document.getElementById('tabLimit').addEventListener('click', () => setMode('limit'));
function setMode(m) { orderMode = m; document.getElementById('tabDeal').classList.toggle('active', m === 'market'); document.getElementById('tabLimit').classList.toggle('active', m === 'limit'); document.getElementById('priceRow').classList.toggle('hidden', m !== 'limit'); updateDealPanel(); }
document.getElementById('lotMinus').addEventListener('click', () => stepLot(-1));
document.getElementById('lotPlus').addEventListener('click', () => stepLot(1));
function stepLot(d) { const i = document.getElementById('lot'); if (sizeUnit === 'usd') i.value = Math.max(1, (parseFloat(i.value) || 0) + d * 10).toFixed(0); else i.value = Math.max(0.001, (parseFloat(i.value) || 0) + d * 0.01).toFixed(3); updateSizeEq(); }
function readSlTp(id) { const sl = parseFloat(document.getElementById('sl').value), tp = parseFloat(document.getElementById('tp').value); return { sl: sl > 0 ? Math.round(sl * pscale(id)) : null, tp: tp > 0 ? Math.round(tp * pscale(id)) : null }; }
// Размер сделки в базовых единицах (учитывает выбор LOT/USD).
function sizeBaseQty(id) { const v = parseFloat(document.getElementById('lot').value) || 0; if (v <= 0) return 0; if (sizeUnit === 'usd') { const p = refPrice(META[id]) / pscale(id); return p > 0 ? v / p : 0; } return v; }
function updateSizeEq() { const it = META[selected]; const el = document.getElementById('sizeEq'); if (!it) { el.textContent = ''; return; } const p = refPrice(it) / pscale(it.id); const v = parseFloat(document.getElementById('lot').value) || 0; const base = it.symbol.split('-')[0]; el.textContent = sizeUnit === 'usd' ? `≈ ${(p > 0 ? v / p : 0).toFixed(qdec(it.id))} ${base}` : `≈ $${(v * p).toLocaleString('en-US', { maximumFractionDigits: 2 })}`; }
function setUnit(u) { if (sizeUnit === u) return; const it = META[selected]; const inp = document.getElementById('lot'); const v = parseFloat(inp.value) || 0; const p = it ? refPrice(it) / pscale(it.id) : 0; if (u === 'usd') { inp.value = (p > 0 ? v * p : 0).toFixed(0); inp.step = '1'; } else { inp.value = (p > 0 ? v / p : 0).toFixed(3); inp.step = '0.001'; } sizeUnit = u; document.getElementById('unitLot').classList.toggle('active', u === 'lot'); document.getElementById('unitUsd').classList.toggle('active', u === 'usd'); updateSizeEq(); }
document.getElementById('unitLot').addEventListener('click', () => setUnit('lot'));
document.getElementById('unitUsd').addEventListener('click', () => setUnit('usd'));
document.getElementById('lot').addEventListener('input', updateSizeEq);
async function submit(side) {
  const id = selected; if (id == null) return; const msg = document.getElementById('dealMsg');
  const baseQty = sizeBaseQty(id);
  if (!(baseQty > 0)) { msg.textContent = 'Enter size'; msg.style.color = 'var(--down)'; return; }
  const { sl, tp } = readSlTp(id); const qty = Math.round(baseQty * qscale(id)); let url, body;
  if (orderMode === 'limit') { const price = parseFloat(document.getElementById('price').value); if (!(price > 0)) { msg.textContent = 'Enter price'; msg.style.color = 'var(--down)'; return; } url = '/pending'; body = { instrument: id, side, qty, price: Math.round(price * pscale(id)), sl, tp }; }
  else { url = '/deals'; body = { instrument: id, side, qty, sl, tp }; }
  const r = await fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token()}` }, body: JSON.stringify(body) });
  const data = await r.json().catch(() => null);
  if (r.ok) { msg.textContent = orderMode === 'limit' ? `Limit order #${data.id}` : `Opened #${data.id}: ${side.toUpperCase()} @ ${fmtP(id, data.entry)}`; msg.style.color = 'var(--up)'; switchBottom(orderMode === 'limit' ? 'pending' : 'deals'); pollAccount(); pollDeals(); pollPending(); }
  else { msg.textContent = `Rejected ${r.status}: ${data?.error || ''}`; msg.style.color = 'var(--down)'; }
}
document.getElementById('btnBuy').addEventListener('click', () => submit('buy'));
document.getElementById('btnSell').addEventListener('click', () => submit('sell'));
document.getElementById('userSel').addEventListener('change', () => { pollAccount(); pollDeals(); pollPending(); pollClosed(); });

// ============================ Account / positions / history ================
async function pollAccount() { try { const r = await fetch('/account', { headers: { Authorization: `Bearer ${token()}` } }); if (!r.ok) return; const a = await r.json(); document.getElementById('mBalance').textContent = usd(a.balance); document.getElementById('mEquity').textContent = usd(a.equity); document.getElementById('mMargin').textContent = usd(a.used_margin); document.getElementById('mFree').textContent = usd(a.free_margin); const p = document.getElementById('mPnl'); p.textContent = usd(a.open_pnl); p.style.color = a.open_pnl > 0 ? 'var(--up)' : a.open_pnl < 0 ? 'var(--down)' : ''; } catch (e) { /* */ } }
async function pollDeals() {
  try { const r = await fetch('/deals', { headers: { Authorization: `Bearer ${token()}` } }); if (!r.ok) return; const list = await r.json(); currentDeals = list.filter((d) => d.instrument === selected); redrawPriceLines();
    document.getElementById('deals').innerHTML = list.map((d) => { const pos = d.pnl >= 0; const lot = (d.qty / 10 ** d.qty_decimals).toFixed(d.qty_decimals); const f = (raw) => (raw / 10 ** d.price_decimals).toFixed(d.price_decimals); const pips = showPips ? ` (${d.side === 'buy' ? d.mark - d.entry : d.entry - d.mark}p)` : ''; return `<div class="deal-row"><span>${d.symbol}</span><span class="sd ${d.side}">${d.side.toUpperCase()}</span><span>${lot}</span><span>${f(d.entry)}</span><span>${f(d.mark)}</span><span class="pnl ${pos ? 'pos' : 'neg'}">${usd(d.pnl)}${pips}</span><button class="closebtn" data-id="${d.id}">Close</button></div>`; }).join('') || '<div class="deal-row muted"><span>No open positions</span></div>';
    document.querySelectorAll('#deals .closebtn').forEach((b) => b.addEventListener('click', () => closeDeal(+b.dataset.id))); } catch (e) { /* */ }
}
async function closeDeal(id) { const r = await fetch(`/deals/${id}/close`, { method: 'POST', headers: { Authorization: `Bearer ${token()}` } }); if (r.ok) { pollAccount(); pollDeals(); pollClosed(); } }
async function pollPending() { try { const r = await fetch('/pending', { headers: { Authorization: `Bearer ${token()}` } }); if (!r.ok) return; const list = await r.json(); document.getElementById('pending').innerHTML = list.map((d) => { const lot = (d.qty / 10 ** d.qty_decimals).toFixed(d.qty_decimals); const f = (raw) => (raw == null ? '—' : (raw / 10 ** d.price_decimals).toFixed(d.price_decimals)); return `<div class="deal-row"><span>${d.symbol}</span><span class="sd ${d.side}">${d.side.toUpperCase()}</span><span>${lot}</span><span>${f(d.price)}</span><span>${f(d.sl)}</span><span>${f(d.tp)}</span><button class="closebtn" data-id="${d.id}">Cancel</button></div>`; }).join('') || '<div class="deal-row muted"><span>No limit orders</span></div>'; document.querySelectorAll('#pending .closebtn').forEach((b) => b.addEventListener('click', () => cancelPending(+b.dataset.id))); } catch (e) { /* */ } }
async function cancelPending(id) { const r = await fetch(`/pending/${id}`, { method: 'DELETE', headers: { Authorization: `Bearer ${token()}` } }); if (r.ok) pollPending(); }
async function pollClosed() { try { const r = await fetch('/deals/closed', { headers: { Authorization: `Bearer ${token()}` } }); if (!r.ok) return; const list = await r.json(); document.getElementById('closed').innerHTML = list.map((d) => { const pos = d.pnl >= 0; const lot = (d.qty / 10 ** d.qty_decimals).toFixed(d.qty_decimals); const f = (raw) => (raw / 10 ** d.price_decimals).toFixed(d.price_decimals); return `<div class="deal-row"><span>${d.symbol}</span><span class="sd ${d.side}">${d.side.toUpperCase()}</span><span>${lot}</span><span>${f(d.entry)}</span><span>${f(d.exit)}</span><span class="pnl ${pos ? 'pos' : 'neg'}">${usd(d.pnl)}</span><span></span></div>`; }).join('') || '<div class="deal-row muted"><span>No history</span></div>'; } catch (e) { /* */ } }

// ============================ Signals ======================================
function computeSignals() { const cont = document.getElementById('signals'); const it = META[selected]; if (!it || rawCandles.length < 30) { cont.innerHTML = ''; return; } const close = rawCandles.map((c) => c.close); const sig = []; const r = rsi(close, 14), rv = r[r.length - 1]; if (rv != null && rv > 70) sig.push(['RSI overbought', 'SELL']); else if (rv != null && rv < 30) sig.push(['RSI oversold', 'BUY']); const f = sma(close, 9), s = sma(close, 21), i = close.length - 1; if (f[i] != null && s[i] != null && f[i - 1] != null && s[i - 1] != null) { if (f[i - 1] <= s[i - 1] && f[i] > s[i]) sig.push(['MA 9/21 cross up', 'BUY']); else if (f[i - 1] >= s[i - 1] && f[i] < s[i]) sig.push(['MA 9/21 cross down', 'SELL']); } cont.innerHTML = sig.map(([n, dir]) => `<div class="deal-row"><span>${it.symbol}</span><span>${n}</span><span>${TFN[tf]}</span><span class="dir ${dir === 'BUY' ? 'buy' : 'sell'} ta-r">${dir}</span></div>`).join('') || '<div class="deal-row muted"><span>No signals on this TF</span></div>'; }

// ============================ Bottom tabs + search + sort ==================
function switchBottom(w) { ['deals', 'pending', 'closed', 'signals'].forEach((n) => { document.getElementById(`tab-${n}`).classList.toggle('active', n === w); document.getElementById(`pane-${n}`).classList.toggle('hidden', n !== w); }); }
document.querySelectorAll('.bottom__tabs span').forEach((t) => t.addEventListener('click', () => switchBottom(t.dataset.pane)));
document.getElementById('search').addEventListener('input', (e) => { const q = e.target.value.toLowerCase(); Object.entries(rowEls).forEach(([id, el]) => { el.style.display = (META[id]?.symbol || '').toLowerCase().includes(q) ? '' : 'none'; }); });
document.getElementById('instruments').addEventListener('click', (e) => { const row = e.target.closest('.irow'); if (!row) return; const id = +row.dataset.id; if (e.target.closest('.star')) { toggleFav(id); return; } selectInstrument(id); });
document.querySelectorAll('.watch__head .sortable').forEach((h) => h.addEventListener('click', () => {
  const k = h.dataset.sort; if (sortKey === k) sortDir = -sortDir; else { sortKey = k; sortDir = -1; }
  document.querySelectorAll('.watch__head .sortable').forEach((x) => x.classList.remove('asc', 'desc'));
  h.classList.add(sortDir === 1 ? 'asc' : 'desc'); reorderRows();
}));

// ============================ Start ========================================
renderIcons();
applyLayout();
applyCols();
initResizers();
refreshInstruments();
setInterval(refreshInstruments, 1000);
setInterval(syncLast, 2000);
setInterval(() => { pollAccount(); pollDeals(); }, 1000);
setInterval(() => { pollPending(); pollClosed(); }, 1500);
connectWS();
pollAccount(); pollDeals(); pollPending(); pollClosed();
