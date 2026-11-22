'use strict';

let META = {}, selected = null, tf = 3600, orderMode = 'market', orderSide = 'buy';
let chartType = 'candles', priceType = 'mid', showPips = false, showSLTP = true, sizeUnit = 'lot';
const overlays = { ma: 1, sma: 0, ema: 0, wma: 0, supertrend: 0, psar: 0, ichimoku: 0, alligator: 0, boll: 0, keltner: 0, donchian: 0, vwap: 0 };
let oscillator = '';
let rawCandles = [], currentDeals = [];

const api = Api.create();
const I18N = window.I18n;
const T = (/** @type {string} */ k) => I18N.t(k);
const tv = (/** @type {string} */ k, /** @type {Record<string, any>} */ v) => I18N.t(k, v);
const wT = (/** @type {string} */ k, /** @type {string} */ en, /** @type {any} */ v) => I18N.widgetT(k, en, v);
let lastAccount = null;
const pdec = (id) => (META[id]?.price_decimals ?? 2);
const qdec = (id) => (META[id]?.qty_decimals ?? 3);
const pscale = (id) => 10 ** pdec(id);
const qscale = (id) => 10 ** qdec(id);
/**
 * @param {any} it
 * @returns {[string, string]}
 */
const pairOf = (it) => Market.pair(it);
/** @param {number} id */
const quoteDriven = (id) => { const it = META[id]; return !!it && typeof it.category === 'string' && it.category !== 'crypto'; };
/** @param {any} c */
const volPoint = (c) => (c.volume == null ? { time: c.time } : { time: c.time, value: c.volume, color: volColor(c) });

const cssChan = (n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim();
const rgbT = (n) => { const [r, g, b] = cssChan(n).split(/\s+/); return `rgb(${r}, ${g}, ${b})`; };
const rgbaT = (n, a) => { const [r, g, b] = cssChan(n).split(/\s+/); return `rgba(${r}, ${g}, ${b}, ${a})`; };
const fmtP = (id, raw) => I18N.num(raw / pscale(id), pdec(id));
const fmtQ = (id, raw) => (raw / qscale(id)).toFixed(qdec(id));
const fmtN = (/** @type {number} */ v, /** @type {number} */ d) => I18N.num(v, d);
const fmtChange = (/** @type {number} */ v) => I18N.change(v);
const usd = (c) => I18N.money(c);
const TFN = { 1: '1s', 60: '1m', 300: '5m', 900: '15m', 1800: '30m', 3600: '1h', 14400: '4h', 86400: '1d', 604800: '1w' };
const settings = Object.assign(
  { shadows: true, shadowCustom: false, shadowColor: '#787b86', hollowColor: '#26a69a', priceLineWidth: 1, addLine: false, addLineWidth: 1, minChange: 'default' },
  JSON.parse(localStorage.getItem('settings') || '{}')
);
const saveSettings = () => localStorage.setItem('settings', JSON.stringify(settings));

const ICONS = {
  candles: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3"><path d="M5 2v3M5 11v3M11 3v3M11 10v3"/><rect x="3.3" y="5" width="3.4" height="6" fill="currentColor" stroke="none"/><rect x="9.3" y="6" width="3.4" height="4" fill="currentColor" stroke="none"/></svg>',
  hollow: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3"><path d="M5 2v3M5 11v3M11 3v3M11 10v3"/><rect x="3.3" y="5" width="3.4" height="6"/><rect x="9.3" y="6" width="3.4" height="4"/></svg>',
  bars: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3"><path d="M5 2v12M2.5 5H5M5 9h2.5M11 3v10M8.5 6H11M11 10h2.5"/></svg>',
  line: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5"><polyline points="2 12 6 7 9 10 14 3"/></svg>',
  area: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3"><path d="M2 12 6 7 9 10 14 3V14H2Z" fill="currentColor" fill-opacity=".3" stroke="none"/><polyline points="2 12 6 7 9 10 14 3" stroke-width="1.5"/></svg>',
  indicators: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><path d="M2 10c2-4 4-4 6 0s4 4 6-1"/><path d="M2 6c2-3 4 3 6-1s4-2 6 1" opacity=".5"/></svg>',
  wallet: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.3"><rect x="2" y="4" width="11.5" height="8.5" rx="1.6"/><path d="M9.6 7.6H13v2.4H9.6a1.2 1.2 0 0 1 0-2.4Z" fill="currentColor" stroke="none"/></svg>',
  caret: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6"><path d="M4 6l4 4 4-4"/></svg>',
  reset: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><path d="M12.5 8a4.5 4.5 0 1 1-1.3-3.2"/><path d="M12.5 2.5V5H10"/></svg>',
  fullscreen: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><path d="M2 6V2h4M14 6V2h-4M2 10v4h4M14 10v4h-4"/></svg>',
  fullscreenExit: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><path d="M6 2v4H2M10 2v4h4M6 14v-4H2M10 14v-4h4"/></svg>',
  settings: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.25"><circle cx="8" cy="8" r="2.1"/><path d="M8 1.6v1.8M8 12.6v1.8M1.6 8h1.8M12.6 8h1.8M3.5 3.5l1.3 1.3M11.2 11.2l1.3 1.3M12.5 3.5l-1.3 1.3M4.8 11.2l-1.3 1.3"/></svg>',
};
function renderIcons(root = document) { root.querySelectorAll('[data-icon]').forEach((el) => { const n = /** @type {HTMLElement} */ (el).dataset.icon; if (ICONS[n]) el.innerHTML = ICONS[n]; }); }

function fmtDT(t) { return new Date(t * 1000).toLocaleString(I18N.locale(), { timeZone: 'UTC', day: '2-digit', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit', hour12: false }) + ' UTC'; }
/**
 * @param {number} id
 * @param {number} t
 * @param {any} c
 * @param {boolean} [forming]
 */
function updateLegend(id, t, c, forming) {
  const rf = (x) => I18N.num(x, pdec(id));
  document.getElementById('lgSym').textContent = META[id]?.symbol || '';
  document.getElementById('lgTf').textContent = TFN[tf] || '';
  document.getElementById('lgTime').textContent = fmtDT(t) + (forming ? ` · ${T('legend.forming')}` : '');
  document.getElementById('lgOhlc').innerHTML = `O <b>${rf(c.open)}</b>  H <b>${rf(c.high)}</b>  L <b>${rf(c.low)}</b>  C <b class="${c.close >= c.open ? 'up' : 'down'}">${rf(c.close)}</b>`;
  const ma = document.getElementById('lgMa');
  if (ma) {
    if (!overlays.ma || !maLegend.arrays.length) ma.innerHTML = '';
    else {
      let i = maLegend.index.get(t);
      if (i == null) i = maLegend.arrays[0].length - 1;
      ma.innerHTML = MA_SPEC.map(([n, color], k) => {
        const v = maLegend.arrays[k][i];
        return `<span style="color:${color}">MA(${n}) ${v == null ? '—' : rf(v)}</span>`;
      }).join('');
    }
  }
}
/** @param {any} b */
function formingDisplayBar(b) {
  const s = pscale(selected), v = qscale(selected);
  const real = { time: b.time, open: b.open / s, high: b.high / s, low: b.low / s, close: b.close / s, volume: b.volume / v };
  if (chartType !== 'heikin') return real;
  const n = rawCandles.length;
  const src = n && rawCandles[n - 1].time === real.time ? rawCandles.slice(0, n - 1).concat([real]) : rawCandles.concat([real]);
  const ha = heikin(src);
  return ha[ha.length - 1];
}
function refreshLegend() { if (selected == null) return; let c, forming = false; const f = priceSeries.forming(); if (f) { c = formingDisplayBar(f); forming = true; } else if (rawCandles.length) { const n = rawCandles.length; c = chartType === 'heikin' ? heikin(rawCandles)[n - 1] : rawCandles[n - 1]; } if (c) updateLegend(selected, c.time, c, forming); }

const { HL2, HLC3, sma, ema, wma, smma, stdev, trueRange, atr, rsi, highest, lowest, stoch, cci, momentum, roc, williams, ao, acOsc, obv, adLine, cmf, mfi, forceIndex, macdFull, adxDI, bollinger, keltner, donchian, superTrend, psar, ichimoku, vwap, alligator, heikin } = window.Indicators;
const toSeries = (arr, t) => arr.map((v, i) => (v == null || !isFinite(v) ? null : { time: t[i], value: v })).filter(Boolean);

const MA_SPEC = [[7, '#f4a261'], [25, '#c77dde'], [99, '#4fa8e8']];
let maLegend = { index: new Map(), arrays: [] };
const OVERLAYS = {
  ma: (cs) => MA_SPEC.map(([n, color]) => ({ color, values: sma(cs.map((c) => c.close), n) })),
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

const chartEl = document.getElementById('chart');
const chart = LightweightCharts.createChart(chartEl, {
  layout: { background: { color: rgbT('--bg-base') }, textColor: rgbT('--text-secondary'), attributionLogo: false },
  localization: { locale: I18N.locale() },
  grid: { vertLines: { color: rgbaT('--text-secondary', '.10') }, horzLines: { color: rgbaT('--text-secondary', '.10') } },
  rightPriceScale: { borderColor: rgbaT('--text-secondary', '.25') },
  timeScale: { borderColor: rgbaT('--text-secondary', '.25'), timeVisible: true, secondsVisible: false },
  crosshair: { mode: LightweightCharts.CrosshairMode.Normal },
});
const volume = chart.addHistogramSeries({ priceFormat: { type: 'volume' }, priceScaleId: '', color: rgbaT('--text-secondary', '.30') });
volume.priceScale().applyOptions({ scaleMargins: { top: 0.85, bottom: 0 } });
const draw = Drawings.mount(chartEl.parentElement, { chart, series: () => mainSeries });

let mainSeries = null, overlaySeries = [], oscSeriesList = [], priceLines = [];
/** @type {{ calc: (cs: any[]) => { values: (number | null)[] }[], series: any[] }[]} */
let indGroups = [];
/** @type {ReturnType<typeof setTimeout> | null} */
let indTimer = null;
/** @type {number | null} */
let indFrom = null;
const IND_EVERY_MS = 250;
const priceSeries = window.Series.create();
const volColor = (c) => (c.close >= c.open ? rgbaT('--positive', '.4') : rgbaT('--negative', '.4'));
const isLineType = () => (chartType === 'line' || chartType === 'area');

function candleColors() {
  const wick = (def) => (settings.shadows ? (settings.shadowCustom ? settings.shadowColor : def) : 'rgba(0,0,0,0)');
  const up = rgbT('--positive'), dn = rgbT('--negative');
  if (chartType === 'hollow') return { upColor: 'rgba(0,0,0,0)', downColor: 'rgba(0,0,0,0)', borderUpColor: settings.hollowColor, borderDownColor: dn, wickUpColor: wick(settings.hollowColor), wickDownColor: wick(dn) };
  return { upColor: up, downColor: dn, borderUpColor: up, borderDownColor: dn, wickUpColor: wick(up), wickDownColor: wick(dn) };
}
function makeMainSeries() {
  if (mainSeries) chart.removeSeries(mainSeries);
  if (chartType === 'candles' || chartType === 'hollow' || chartType === 'heikin') mainSeries = chart.addCandlestickSeries(candleColors());
  else if (chartType === 'bars') mainSeries = chart.addBarSeries({ upColor: rgbT('--positive'), downColor: rgbT('--negative') });
  else if (chartType === 'line') mainSeries = chart.addLineSeries({ color: rgbT('--accent'), lineWidth: 2 });
  else mainSeries = chart.addAreaSeries({ lineColor: rgbT('--accent'), topColor: rgbaT('--accent', '.25'), bottomColor: rgbaT('--accent', '.02') });
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
function buildMaLegend(times) {
  maLegend = overlays.ma
    ? { index: new Map(times.map((t, i) => [t, i])), arrays: MA_SPEC.map(([n]) => sma(rawCandles.map((c) => c.close), n)) }
    : { index: new Map(), arrays: [] };
}
function applyIndicators() {
  if (indTimer) clearTimeout(indTimer);
  indTimer = null; indFrom = null; indGroups = [];
  overlaySeries.forEach((s) => chart.removeSeries(s)); overlaySeries = [];
  oscSeriesList.forEach((s) => chart.removeSeries(s)); oscSeriesList = [];
  const times = rawCandles.map((c) => c.time);
  buildMaLegend(times);
  for (const key in overlays) {
    if (!overlays[key] || !OVERLAYS[key]) continue;
    const group = { calc: OVERLAYS[key], series: [] };
    for (const ln of OVERLAYS[key](rawCandles)) { const s = chart.addLineSeries({ color: ln.color, lineWidth: ln.width || 1, priceLineVisible: false, lastValueVisible: false }); s.setData(toSeries(ln.values, times)); overlaySeries.push(s); group.series.push(s); }
    indGroups.push(group);
  }
  if (oscillator && OSC[oscillator]) {
    volume.applyOptions({ visible: false });
    const group = { calc: OSC[oscillator], series: [] };
    OSC[oscillator](rawCandles).forEach((ln, i) => { const s = chart.addLineSeries({ color: ln.color, lineWidth: 1, priceScaleId: 'osc', priceLineVisible: false, lastValueVisible: false }); if (i === 0) s.priceScale().applyOptions({ scaleMargins: { top: 0.78, bottom: 0 } }); s.setData(toSeries(ln.values, times)); oscSeriesList.push(s); group.series.push(s); });
    indGroups.push(group);
  } else volume.applyOptions({ visible: true });
}
/** @param {number} time */
function queueLiveIndicators(time) {
  indFrom = indFrom == null ? time : Math.min(indFrom, time);
  if (!indTimer) indTimer = setTimeout(flushLiveIndicators, IND_EVERY_MS);
}
function flushLiveIndicators() {
  const from = indFrom;
  indTimer = null; indFrom = null;
  if (from == null || !rawCandles.length) return;
  const times = rawCandles.map((c) => c.time);
  const i0 = LiveBars.indexFrom(rawCandles, from);
  for (const g of indGroups) {
    const lines = g.calc(rawCandles);
    g.series.forEach((s, k) => { if (lines[k]) for (const p of LiveBars.tail(lines[k].values, times, i0)) s.update(p); });
  }
  buildMaLegend(times);
  refreshLegend();
}
function redrawPriceLines() {
  if (!mainSeries) return;
  priceLines.forEach((l) => mainSeries.removePriceLine(l)); priceLines = [];
  const s = pscale(selected);
  if (showSLTP) for (const d of currentDeals) {
    priceLines.push(mainSeries.createPriceLine({ price: d.entry / s, color: '#8899aa', lineStyle: 2, lineWidth: 1, title: d.side === 'buy' ? 'L' : 'S' }));
    if (d.sl != null) priceLines.push(mainSeries.createPriceLine({ price: d.sl / s, color: rgbT('--negative'), lineWidth: 1, title: 'SL' }));
    if (d.tp != null) priceLines.push(mainSeries.createPriceLine({ price: d.tp / s, color: rgbT('--positive'), lineWidth: 1, title: 'TP' }));
  }
  if (settings.addLine && rawCandles.length > 1) priceLines.push(mainSeries.createPriceLine({ price: rawCandles[rawCandles.length - 2].close, color: rgbT('--accent'), lineStyle: 3, lineWidth: Math.max(1, settings.addLineWidth || 1), title: 'ref' }));
}

new ResizeObserver(() => chart.applyOptions({ width: chartEl.clientWidth, height: chartEl.clientHeight })).observe(chartEl);
chart.subscribeCrosshairMove((p) => { const id = selected; if (id == null) return; const d = p.seriesData?.get(mainSeries); if (p.time && d) { const c = d.close != null ? d : { open: d.value, high: d.value, low: d.value, close: d.value }; const f = priceSeries.forming(); updateLegend(id, p.time, c, !!f && p.time === f.time); } else refreshLegend(); });

async function loadCandles(id, timeframe) {
  const load = document.getElementById('loading'); load.classList.remove('hidden');
  try {
    const fetched = await api.candles(id, timeframe, 300);
    markMarketData('candles', fetched !== null);
    if (fetched === null) {
      load.classList.add('hidden');
      return;
    }
    const raw = fetched;
    const s = pscale(id), v = qscale(id);
    rawCandles = raw.map((c) => ({ time: c.time, open: c.open / s, high: c.high / s, low: c.low / s, close: c.close / s, volume: c.volume == null ? null : c.volume / v }));
    makeMainSeries();
    mainSeries.setData(mainData());
    volume.setData(rawCandles.map(volPoint));
    applyIndicators(); redrawPriceLines(); panel.refreshSignals();
    priceSeries.seed(rawCandles.length ? raw[raw.length - 1] : null);
    if (rawCandles.length) { const n = rawCandles.length; chart.timeScale().setVisibleLogicalRange({ from: Math.max(0, n - 90), to: n + 3 }); }
    refreshLegend();
    draw.clear();
  } finally { load.classList.add('hidden'); }
}
/** @param {any} [bar] */
function drawForming(bar) {
  const b = bar || priceSeries.forming();
  if (!b) return;
  const s = pscale(selected), v = qscale(selected);
  const n = rawCandles.length;
  const prev = n ? rawCandles[n - 1].time : b.time;
  const vol = b.volume == null || (quoteDriven(selected) && !b.volume) ? null : b.volume / v;
  const kind = LiveBars.upsert(rawCandles, { time: b.time, open: b.open / s, high: b.high / s, low: b.low / s, close: b.close / s, volume: vol });
  if (kind === 'old') return;
  queueLiveIndicators(kind === 'new' ? prev : b.time);
  volume.update(volPoint({ time: b.time, open: b.open, close: b.close, volume: vol }));
  if (!mainSeries) return;
  const d = formingDisplayBar(b);
  mainSeries.update(isLineType() ? { time: d.time, value: d.close } : { time: d.time, open: d.open, high: d.high, low: d.low, close: d.close });
  refreshLegend();
}
let quoteRedraw = null;
/** @param {{ instrument: number, bid: number|null, ask: number|null, price: number, time: number|null }} q */
function onLiveQuote(q) {
  const it = META[q.instrument];
  if (!it) return;
  if (q.bid !== null) it.bid = q.bid;
  if (q.ask !== null) it.ask = q.ask;
  if (it.has_trades === false || it.last == null) it.last = q.price;
  if (q.instrument !== selected) return;
  if (quoteDriven(q.instrument)) {
    const now = Math.floor((q.time !== null ? q.time : Date.now()) / 1000);
    const b = priceSeries.onTrade({ now, tf, price: q.price, qty: 0 });
    if (b) drawForming(b);
  }
  if (!quoteRedraw) quoteRedraw = setTimeout(() => { quoteRedraw = null; updateDealPanel(); }, 250);
}
/** @param {{ instrument: number, market: any }} m */
function onMarketStatus(m) {
  const it = META[m.instrument];
  if (!it) return;
  it.market = m.market;
  watchlist.update(m.instrument, { market: m.market });
  if (m.instrument === selected) updateDealPanel();
  panel.refreshDeals();
}
function onLiveTrade(id, price, qty, timeMs) { if (id !== selected) return; const now = Number.isFinite(timeMs) ? Math.floor(timeMs / 1000) : Math.floor(Date.now() / 1000); const b = priceSeries.onTrade({ now, tf, price, qty }); if (b) drawForming(b); }
async function syncLast() {
  const id = selected; if (id == null) return;
  if (priceSeries.isBroken()) { await loadCandles(id, tf); return; }
  const raw = (await api.candles(id, tf, 2)) || []; if (!raw.length) return;
  const last = raw[raw.length - 1];
  if (last.time >= priceSeries.lastBarTime()) { priceSeries.seed(last); drawForming(); }
}

function refPrice(it) { if (!it || it.bid == null || it.ask == null) return it?.last; if (priceType === 'ask') return it.ask; if (priceType === 'bid') return it.bid; return Math.round((it.bid + it.ask) / 2); }
function updateDealPanel() {
  const it = META[selected]; if (!it) return;
  document.getElementById('dealSym').textContent = it.symbol;
  const ref = refPrice(it);
  document.getElementById('dealSub').textContent = `${priceType.toUpperCase()} ${ref != null ? fmtP(it.id, ref) : '—'}`;
  const chg = document.getElementById('dealChange');
  const chgKnown = it.change != null;
  const chgUp = chgKnown && it.change >= 0;
  chg.textContent = chgKnown ? I18N.change(it.change) : '—';
  chg.className = `deal__change${chgKnown ? (chgUp ? ' up' : ' down') : ''}`;
  const mk = tariff.markup_bps;
  document.getElementById('askPx').textContent = it.ask != null ? fmtP(it.id, Tariff.execPrice(true, it.ask, mk)) : '—';
  document.getElementById('bidPx').textContent = it.bid != null ? fmtP(it.id, Tariff.execPrice(false, it.bid, mk)) : '—';
  updateFeeInfo();
  document.getElementById('watermark').textContent = it.symbol;
  const pips = document.getElementById('pipsInfo');
  if (showPips && it.bid != null && it.ask != null) { pips.classList.remove('hidden'); pips.textContent = tv('f.spread', { n: I18N.num(it.ask - it.bid) }); } else pips.classList.add('hidden');
  const quote = pairOf(it)[1];
  document.getElementById('priceUnit').textContent = quote;
  if (orderMode === 'limit' && !/** @type {HTMLInputElement} */ (document.getElementById('price')).value && ref != null) /** @type {HTMLInputElement} */ (document.getElementById('price')).value = (ref / pscale(it.id)).toFixed(pdec(it.id));
  updateSizeEq();
  refreshStops();
  renderInstrumentHeader(it);
}

const compactNum = (/** @type {number} */ v) => {
  const a = Math.abs(v);
  if (a >= 1e9) return I18N.num(v / 1e9, 2) + 'B';
  if (a >= 1e6) return I18N.num(v / 1e6, 2) + 'M';
  if (a >= 1e3) return I18N.num(v / 1e3, 2) + 'K';
  return I18N.num(v, 2);
};
let ihPrev = { id: null, last: null, dir: '' };
/** @param {any} it */
function renderInstrumentHeader(it) {
  const [base, quote] = pairOf(it);
  const $ = (/** @type {string} */ id) => document.getElementById(id);
  $('ihBase').textContent = base;
  $('ihQuote').textContent = '/' + (quote || '');
  const coin = $('ihCoin');
  if (coin.dataset.base !== base) {
    coin.dataset.base = base;
    coin.innerHTML = `<img src="/vendor/coins/${base.toLowerCase()}.svg" alt="" onerror="this.remove()">`;
  }
  if (ihPrev.id !== it.id) ihPrev = { id: it.id, last: null, dir: '' };
  if (it.last != null && ihPrev.last != null && it.last !== ihPrev.last) ihPrev.dir = it.last > ihPrev.last ? 'up' : 'down';
  if (it.last != null) ihPrev.last = it.last;
  const last = $('ihLast');
  last.textContent = it.last != null ? fmtP(it.id, it.last) : '—';
  last.className = 'ih-last ' + ihPrev.dir;
  $('ihMid').textContent = it.bid != null && it.ask != null ? `${fmtP(it.id, it.bid)} / ${fmtP(it.id, it.ask)}` : '';
  const chg = $('ihChg');
  const known = it.change != null;
  chg.textContent = known ? I18N.change(it.change) : '—';
  chg.className = 'ih-v ' + (known ? (it.change >= 0 ? 'up' : 'down') : '');
  $('ihHigh').textContent = it.high != null ? fmtP(it.id, it.high) : '—';
  $('ihLow').textContent = it.low != null ? fmtP(it.id, it.low) : '—';
  $('ihVolL').textContent = tv('ih.vol', { c: base });
  $('ihQVolL').textContent = tv('ih.vol', { c: quote || '' });
  $('ihVol').textContent = it.volume != null ? compactNum(it.volume / qscale(it.id)) : '—';
  $('ihQVol').textContent = it.quote_volume != null ? compactNum(it.quote_volume / pscale(it.id)) : '—';
  $('ihSpread').textContent = it.bid != null && it.ask != null ? fmtP(it.id, it.ask - it.bid) : '—';
  const mkText = marketText(it);
  const only = Market.closeOnly(it);
  const mk = $('ihMarket');
  mk.hidden = !mkText && !only;
  mk.textContent = [mkText, only ? T('mk.closeOnly') : null].filter(Boolean).join(' · ');
  mk.title = only && !mkText ? T('gate.closeOnly') : mk.textContent;
  mk.classList.toggle('ih-market--only', only && !mkText);
  document.body.classList.toggle('src-v2', quoteDriven(it.id));
}

/**
 * @param {any} it
 * @returns {string|null}
 */
function marketText(it) {
  const st = Market.state(it).status;
  if (st === 'halted') return T('mk.halted');
  if (st !== 'closed') return null;
  const now = Date.now();
  const at = Market.nextOpen(it, now);
  return at ? tv('mk.closedAt', { when: Market.when(at, I18N.locale(), now) }) : T('mk.closed');
}
/** @param {any} it */
function marketNote(it) {
  const st = Market.state(it).status;
  const only = Market.closeOnly(it);
  if (st === 'closed' || st === 'halted') {
    return { kind: st, text: T(`mk.${st}.short`), tip: [marketText(it), only ? T('mk.closeOnly') : null].filter(Boolean).join(' · ') };
  }
  return only ? { kind: 'closeOnly', text: T('mk.closeOnly.short'), tip: T('gate.closeOnly') } : null;
}
/**
 * @param {any} it
 * @param {number} raw
 */
const sizeLabel = (it, raw) => `${Sizing.format(it, raw, fmtN)} ${Sizing.contract(it) ? T('sz.lot') : pairOf(it)[0]}`;
/** @param {any} d */
function dealSize(d) {
  const it = META[d.instrument];
  if (!Sizing.contract(it)) return null;
  const spec = Object.assign({}, it, Number.isInteger(d.qty_decimals) ? { qty_decimals: d.qty_decimals } : {});
  return sizeLabel(spec, d.qty);
}
/**
 * @param {number} status
 * @param {any} data
 */
function refusal(status, data) {
  const at = data && Number.isFinite(data.next_open) && data.next_open > 0 ? data.next_open : null;
  return rejectionText(status, data?.error) + (at ? ' ' + tv('mk.opens', { when: Market.when(at, I18N.locale(), Date.now()) }) : '');
}

/** @param {any[]} list */
function renderTape(list) {
  const track = document.getElementById('tapeTrack');
  if (!track || !list.length) return;
  const html = list.map((it) => {
    const [b, q] = pairOf(it);
    const known = it.change != null;
    const up = known && it.change >= 0;
    return `<span class="tape__it" data-id="${it.id}"><b>${b}/${q || ''}</b><span>${it.last != null ? fmtP(it.id, it.last) : '—'}</span>` +
      `<span class="${known ? (up ? 'up' : 'down') : 'muted'}">${known ? I18N.change(it.change) : '—'}</span></span>`;
  }).join('');
  track.innerHTML = html + html;
  track.style.animationDuration = `${Math.max(30, list.length * 5)}s`;
}

async function showStorageMode() {
  const h = await api.health();
  const el = document.getElementById('volatileWarn');
  if (!el || !h) return;
  el.hidden = h.durable !== false;
  el.textContent = T('state.volatile');
  el.title = `storage: ${h.storage}`;
}

const marketDown = { candles: false, depth: false, stats: false, instruments: false };
function markMarketData(kind, ok) {
  if (marketDown[kind] === !ok) return;
  marketDown[kind] = !ok;
  renderMarketWarn();
}
function renderMarketWarn() {
  const el = document.getElementById('dataWarn');
  if (!el) return;
  const down = Object.keys(marketDown).filter((k) => marketDown[k]);
  el.hidden = down.length === 0;
  el.textContent = down.length ? tv('data.down', { what: down.map((k) => T('data.' + k)).join(', ') }) : '';
}

const book = OrderBook.mount(document.getElementById('book'), {
  instrument: () => selected,
  fetch: async (id, limit) => { const r = await api.depthDetailed(id, limit); markMarketData('depth', r.reason === null); return r; },
  onPick: (price) => { setMode('limit'); const inp = /** @type {HTMLInputElement} */ (document.getElementById('price')); inp.value = price; inp.classList.add('flash'); setTimeout(() => inp.classList.remove('flash'), 400); },
  symbols: (id) => { const [base, quote] = pairOf(META[id]); return { base, quote }; },
  unavailable: (id) => (META[id]?.has_depth === false ? T('ob.unavailable') : null),
  markPrice: (id) => (META[id]?.last != null ? META[id].last / pscale(id) : null),
  interval: 700,
  storagePrefix: 'ob',
  source: 'Binance',
  t: wT,
  num: fmtN,
});
const stats = AssetStats.mount(document.getElementById('statbar'), {
  instrument: () => selected,
  fetch: async (id) => { const r = await api.statsDetailed(id); markMarketData('stats', r.reason === null); return r; },
  labels: TFN,
  change: fmtChange,
  interval: 5000,
});
const panel = Positions.mount(document.getElementById('bottom'), {
  api,
  fmtUsd: usd,
  pips: () => showPips,
  onDeals: (list) => { currentDeals = list.filter((d) => d.instrument === selected); redrawPriceLines(); },
  onChange: () => pollAccount(),
  levels: { enabled: () => sltpEdit && !(lastAccount && lastAccount.trading_frozen), edit: openLevels },
  size: dealSize,
  closeBlock: (d) => { const why = Market.blockClose(META[d.instrument]); return why ? T('gate.close.' + why) : null; },
  onReject: (status, data) => { const msg = document.getElementById('dealMsg'); msg.textContent = refusal(status, data); msg.style.color = 'var(--down)'; },
  signals: () => computeSignals(),
  balance: () => balanceRows(),
  t: wT,
  num: fmtN,
  interval: 1500,
});
const trades = Trades.mount(document.getElementById('trades'), {
  instrument: () => selected,
  decimals: (id) => ({ pd: pdec(id), qd: qdec(id) }),
  myTrades: async (id) => {
    const all = await api.closedDeals();
    return all === null ? null : all.filter((d) => d.instrument === id);
  },
  fmtUsd: usd,
  source: 'Binance',
  feed: (id) => META[id]?.has_trades !== false,
  size: dealSize,
  t: wT,
  num: fmtN,
  interval: 2500,
});
const SITE = new URLSearchParams(location.search).get('site') || '';
const WANT_SYMBOL = new URLSearchParams(location.search).get('symbol') || '';
const watchlist = Watchlist.mount(document.querySelector('.watch'), {
  fetch: async () => {
    const list = await api.instruments(SITE);
    markMarketData('instruments', list !== null);
    return list;
  },
  instrument: () => selected,
  onData: (list) => { list.forEach((it) => (META[it.id] = it)); updateDealPanel(); renderTape(list); },
  onSelect: (id) => { document.body.classList.remove('m-watch'); selectInstrument(id); },
  interval: 1000,
  colsKey: 'watch.cols.v2',
  columnsDefault: { last: 1, change: 1, sell: 0, buy: 0, spread: 0, high: 0 },
  title: '',
  t: wT,
  num: fmtN,
  change: fmtChange,
  note: marketNote,
  initial: (list) => Market.matchSymbol(list, WANT_SYMBOL),
});
/** @param {any} it */
function syncTimeframes(it) {
  const one = /** @type {HTMLButtonElement} */ (document.querySelector('#tfs button[data-tf="1"]'));
  if (!one) return;
  const off = !!it && it.has_1s === false;
  one.disabled = off;
  one.title = off ? T('tf.unsupported') : '';
  if (!off || tf !== 1) return;
  tf = 60;
  chart.applyOptions({ timeScale: { secondsVisible: false } });
  document.querySelectorAll('#tfs button').forEach((b) => b.classList.toggle('active', /** @type {HTMLElement} */ (b).dataset.tf === '60'));
}
async function selectInstrument(id) { selected = id; watchlist.setActive(id); syncTimeframes(META[id]); syncLotStep(); /** @type {HTMLInputElement} */ (document.getElementById('price')).value = ''; resetStops(); await loadCandles(id, tf); updateDealPanel(); pollAccount(); panel.refreshDeals(); book.reset(); book.refresh(); stats.refresh(); trades.clearTape(); trades.refreshMine(); }
document.getElementById('tfs').addEventListener('click', (e) => { const btn = /** @type {Element} */ (e.target).closest('button'); if (!btn || btn.disabled) return; tf = +btn.dataset.tf; chart.applyOptions({ timeScale: { secondsVisible: tf < 60 } }); document.querySelectorAll('#tfs button').forEach((b) => b.classList.toggle('active', b === btn)); if (selected != null) loadCandles(selected, tf); });

function closeDD() { document.querySelectorAll('.dd').forEach((d) => d.classList.add('hidden')); }
function toggleDD(id) { const m = document.getElementById(id); const wasHidden = m.classList.contains('hidden'); closeDD(); if (wasHidden) m.classList.remove('hidden'); }
document.addEventListener('click', (e) => { if (!/** @type {Element} */ (e.target).closest('.tb-dd')) closeDD(); });

document.getElementById('ctBtn').addEventListener('click', () => toggleDD('ctMenu'));
document.getElementById('ctMenu').addEventListener('click', (e) => { const it = /** @type {Element} */ (e.target).closest('[data-ct]'); if (!it) return; chartType = /** @type {HTMLElement} */ (it).dataset.ct; document.querySelectorAll('#ctMenu .dd-item').forEach((x) => x.classList.toggle('active', x === it)); const icName = (/** @type {HTMLElement} */ (it.querySelector('[data-icon]')))?.dataset.icon || 'candles'; const btnIc = /** @type {HTMLElement} */ (document.querySelector('#ctBtn .ic')); if (btnIc) { btnIc.dataset.icon = icName; btnIc.innerHTML = ICONS[icName]; } const ctl = document.getElementById('ctLabel'); ctl.dataset.i18n = `ct.${chartType}`; ctl.textContent = T(ctl.dataset.i18n); closeDD(); makeMainSeries(); mainSeries.setData(mainData()); applyIndicators(); redrawPriceLines(); });

document.getElementById('indBtn').addEventListener('click', () => toggleDD('indMenu'));
document.getElementById('indMenu').addEventListener('click', (e) => {
  const ov = /** @type {Element} */ (e.target).closest('[data-ov]');
  const os = /** @type {Element} */ (e.target).closest('[data-osc]');
  if (ov) { const k = /** @type {HTMLElement} */ (ov).dataset.ov; overlays[k] = overlays[k] ? 0 : 1; ov.classList.toggle('on', !!overlays[k]); applyIndicators(); }
  else if (os) { oscillator = /** @type {HTMLElement} */ (os).dataset.osc; document.querySelectorAll('#indMenu .osc').forEach((x) => x.classList.toggle('on', x === os)); applyIndicators(); }
});

document.getElementById('ptBtn').addEventListener('click', () => toggleDD('ptMenu'));
document.getElementById('ptMenu').addEventListener('click', (e) => { const it = /** @type {Element} */ (e.target).closest('[data-pt]'); if (!it) return; priceType = /** @type {HTMLElement} */ (it).dataset.pt; document.querySelectorAll('#ptMenu .dd-item').forEach((x) => x.classList.toggle('active', x === it)); document.getElementById('ptLabel').textContent = it.textContent.trim().toUpperCase(); closeDD(); updateDealPanel(); });

document.getElementById('sltpBtn').addEventListener('click', () => { showSLTP = !showSLTP; document.getElementById('sltpBtn').classList.toggle('on', showSLTP); redrawPriceLines(); });
document.getElementById('pipsBtn').addEventListener('click', () => { showPips = !showPips; document.getElementById('pipsBtn').classList.toggle('on', showPips); updateDealPanel(); panel.refreshDeals(); });

function resetChart() {
  chartType = 'candles';
  Object.keys(overlays).forEach((k) => (overlays[k] = 0));
  oscillator = '';
  document.querySelectorAll('#indMenu [data-ov]').forEach((x) => x.classList.remove('on'));
  document.querySelectorAll('#indMenu .osc').forEach((x) => x.classList.toggle('on', /** @type {HTMLElement} */ (x).dataset.osc === ''));
  document.querySelectorAll('#ctMenu .dd-item').forEach((x) => x.classList.toggle('active', /** @type {HTMLElement} */ (x).dataset.ct === 'candles'));
  const btnIc = document.querySelector('#ctBtn .ic'); if (btnIc) { /** @type {HTMLElement} */ (btnIc).dataset.icon = 'candles'; btnIc.innerHTML = ICONS.candles; }
  const ctl = document.getElementById('ctLabel'); ctl.dataset.i18n = 'ct.candles'; ctl.textContent = T('ct.candles');
  makeMainSeries(); mainSeries.setData(mainData()); applyIndicators(); redrawPriceLines();
  const n = rawCandles.length; if (n) chart.timeScale().setVisibleLogicalRange({ from: Math.max(0, n - 90), to: n + 3 });
}
document.getElementById('resetBtn').addEventListener('click', resetChart);
const chartSection = /** @type {HTMLElement} */ (document.querySelector('.center'));
const isChartMax = () => document.fullscreenElement === chartSection || chartSection.classList.contains('is-max');
function syncFullBtn() {
  const on = isChartMax();
  const b = document.getElementById('fullBtn');
  b.classList.toggle('on', on);
  const key = on ? 'tb.fullExit' : 'tb.full';
  b.dataset.i18nTip = key; b.dataset.tip = T(key);
  b.setAttribute('aria-pressed', String(on));
  const ic = /** @type {HTMLElement} */ (b.querySelector('.ic'));
  if (ic) { ic.dataset.icon = on ? 'fullscreenExit' : 'fullscreen'; ic.innerHTML = ICONS[ic.dataset.icon]; }
  document.body.classList.toggle('chart-max', on);
  requestAnimationFrame(() => chart.applyOptions({ width: chartEl.clientWidth, height: chartEl.clientHeight }));
}
/** @param {boolean} on */
function setChartMax(on) {
  if (on === isChartMax()) return;
  if (!on) {
    if (document.fullscreenElement) document.exitFullscreen().catch(() => {});
    chartSection.classList.remove('is-max');
    syncFullBtn();
    return;
  }
  const fallback = () => { chartSection.classList.add('is-max'); syncFullBtn(); };
  if (document.fullscreenEnabled && chartSection.requestFullscreen) {
    chartSection.requestFullscreen().then(syncFullBtn, fallback);
  } else fallback();
}
document.getElementById('fullBtn').addEventListener('click', () => setChartMax(!isChartMax()));
document.addEventListener('fullscreenchange', syncFullBtn);
document.addEventListener('keydown', (e) => {
  if (e.key !== 'Escape') return;
  if (chartSection.classList.contains('is-max')) { chartSection.classList.remove('is-max'); syncFullBtn(); }
  document.body.classList.remove('m-watch');
});
document.getElementById('setBtn').addEventListener('click', () => toggleDD('setMenu'));
function bindSetting(id, key, kind) {
  const el = document.getElementById(id); if (!el) return;
  if (kind === 'bool') /** @type {HTMLInputElement} */ (el).checked = settings[key]; else /** @type {HTMLInputElement} */ (el).value = settings[key];
  el.addEventListener('change', () => { settings[key] = kind === 'bool' ? /** @type {HTMLInputElement} */ (el).checked : (kind === 'num' ? Number(/** @type {HTMLInputElement} */ (el).value) : /** @type {HTMLInputElement} */ (el).value); saveSettings(); applySettings(); });
}
bindSetting('sHollow', 'hollowColor', 'str'); bindSetting('sShadows', 'shadows', 'bool'); bindSetting('sShadowCustom', 'shadowCustom', 'bool'); bindSetting('sShadowColor', 'shadowColor', 'str');
bindSetting('sPlw', 'priceLineWidth', 'num'); bindSetting('sAddLine', 'addLine', 'bool'); bindSetting('sAlw', 'addLineWidth', 'num'); bindSetting('sMinChange', 'minChange', 'str');

function bindCol(id, key) { const el = document.getElementById(id); if (!el) return; /** @type {HTMLInputElement} */ (el).checked = !!watchlist.columns()[key]; el.addEventListener('change', () => watchlist.setColumns({ [key]: /** @type {HTMLInputElement} */ (el).checked ? 1 : 0 })); }
bindCol('cLast', 'last'); bindCol('cChange', 'change'); bindCol('cSell', 'sell'); bindCol('cBuy', 'buy'); bindCol('cSpread', 'spread'); bindCol('cHigh', 'high');

const wide = window.innerWidth >= 1800, tall = window.innerHeight >= 1000;
const layout = Object.assign({ leftW: wide ? 320 : 280, rightW: wide ? 340 : 312, bottomH: tall ? 240 : 196, tradesH: tall ? 300 : 230 },
  JSON.parse(localStorage.getItem('layout.v3') || '{}'));
function applyLayout() {
  const g = document.getElementById('grid');
  g.style.setProperty('--lw', layout.leftW + 'px');
  g.style.setProperty('--rw', layout.rightW + 'px');
  g.style.setProperty('--bh', layout.bottomH + 'px');
  document.getElementById('trades').style.height = layout.tradesH + 'px';
}
function initResizers() {
  document.querySelectorAll('[data-rsz]').forEach((h) => h.addEventListener('mousedown', (e) => {
    e.preventDefault(); h.classList.add('drag');
    const type = /** @type {HTMLElement} */ (h).dataset.rsz, sx = /** @type {MouseEvent} */ (e).clientX, sy = /** @type {MouseEvent} */ (e).clientY, sl = layout.leftW, sr = layout.rightW, sb = layout.bottomH, str = layout.tradesH;
    const mv = (ev) => {
      if (type === 'left') layout.leftW = Math.min(520, Math.max(220, sl + (ev.clientX - sx)));
      else if (type === 'right') layout.rightW = Math.min(480, Math.max(280, sr - (ev.clientX - sx)));
      else if (type === 'bottom') layout.bottomH = Math.min(window.innerHeight - 360, Math.max(90, sb - (ev.clientY - sy)));
      else if (type === 'trades') layout.tradesH = Math.min(window.innerHeight - 260, Math.max(90, str - (ev.clientY - sy)));
      applyLayout();
    };
    const up = () => { document.removeEventListener('mousemove', mv); document.removeEventListener('mouseup', up); document.body.style.userSelect = ''; h.classList.remove('drag'); localStorage.setItem('layout.v3', JSON.stringify(layout)); };
    document.body.style.userSelect = 'none';
    document.addEventListener('mousemove', mv); document.addEventListener('mouseup', up);
  }));
}

const tip = document.getElementById('tip');
document.addEventListener('mouseover', (e) => { const t = /** @type {Element} */ (e.target).closest('[data-tip]'); if (!t) return; tip.textContent = /** @type {HTMLElement} */ (t).dataset.tip; tip.classList.remove('hidden'); const r = t.getBoundingClientRect(); tip.style.left = r.left + r.width / 2 + 'px'; tip.style.top = r.bottom + 7 + 'px'; });
document.addEventListener('mouseout', (e) => { if (/** @type {Element} */ (e.target).closest('[data-tip]')) tip.classList.add('hidden'); });

const FEED_STALE_MS = 20000;
let lastFeedMsgAt = 0;
let wsOpen = false;
let wasFeedLive = false;
let feedDropped = 0;
const feedDiag = window.Diag.reporter({ subject: 'feed', every: 50 });

/** @param {string} reason */
function breakPriceSeries(reason) {
  if (priceSeries.isBroken()) return;
  priceSeries.markBreak(reason);
  book.reset();
}

function renderFeedStatus() {
  const st = document.getElementById('status');
  const age = lastFeedMsgAt ? Date.now() - lastFeedMsgAt : Infinity;
  const live = wsOpen && age < FEED_STALE_MS;
  if (lastFeedMsgAt && wasFeedLive && !live) breakPriceSeries('feed-stale');
  wasFeedLive = live;
  const why = !wsOpen
    ? T('feed.gw')
    : live
      ? T('feed.ok')
      : tv('feed.silent', { age: lastFeedMsgAt ? Math.round(age / 1000) + 's' : T('feed.session') });
  if (st) {
    st.className = 'status ' + (live ? 'status--on' : 'status--off');
    st.title = feedDropped ? `${why}; не разобрано сообщений: ${feedDropped}` : why;
  }
  const label = document.getElementById('feedLabel');
  if (label) {
    label.textContent = live ? T('feed.live') : T('feed.nodata');
    label.classList.toggle('up', live);
    label.title = why;
  }
}
function connectWS() {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${proto}://${location.host}/stream`);
  ws.onopen = () => { wsOpen = true; renderFeedStatus(); };
  ws.onclose = () => { wsOpen = false; breakPriceSeries('feed-lost'); renderFeedStatus(); setTimeout(connectWS, 1500); };
  ws.onmessage = (ev) => {
    let e;
    try {
      e = JSON.parse(ev.data);
    } catch (err) {
      feedDropped += 1;
      feedDiag.fail('malformed', String(ev.data).slice(0, 200));
      renderFeedStatus();
      return;
    }
    lastFeedMsgAt = Date.now();
    renderFeedStatus();
    for (const x of e) {
      if (x.type === 'trade') { if (!quoteDriven(x.instrument)) onLiveTrade(x.instrument, x.price, x.qty, x.time); trades.pushMarket(x); }
      else if (x.type === 'quote') { const q = Market.quoteOf(x); if (q) onLiveQuote(q); }
      else if (x.type === 'market') { const m = Market.statusOf(x); if (m) onMarketStatus(m); }
    }
  };
}
setInterval(renderFeedStatus, 2000);

document.getElementById('tabDeal').addEventListener('click', () => setMode('market'));
document.getElementById('tabLimit').addEventListener('click', () => setMode('limit'));
function setMode(m) { orderMode = m; document.getElementById('tabDeal').classList.toggle('active', m === 'market'); document.getElementById('tabLimit').classList.toggle('active', m === 'limit'); document.getElementById('deal').dataset.mode = m; updateDealPanel(); }
function setSide(/** @type {'buy'|'sell'} */ side) {
  if (orderSide !== side) { orderSide = side; resetStops(); }
  document.getElementById('deal').dataset.side = side;
  document.querySelectorAll('.side [data-side]').forEach((b) => b.classList.toggle('active', /** @type {HTMLElement} */ (b).dataset.side === side));
  refreshStops();
}
document.querySelector('.side').addEventListener('click', (e) => {
  const b = /** @type {HTMLElement} */ (/** @type {Element} */ (e.target).closest('[data-side]'));
  if (b) setSide(/** @type {'buy'|'sell'} */ (b.dataset.side));
});

let minStopBps = 0;
const stopAuto = { sl: true, tp: true };
const stopInput = (/** @type {'sl'|'tp'} */ k) => /** @type {HTMLInputElement} */ (document.getElementById(k));
function stopRef(it) {
  if (!it) return null;
  if (orderMode === 'limit') {
    const v = parseFloat(/** @type {HTMLInputElement} */ (document.getElementById('price')).value);
    return v > 0 ? Math.round(v * pscale(it.id)) : null;
  }
  const raw = orderSide === 'buy' ? it.ask : it.bid;
  return Number.isInteger(raw) && raw > 0 ? raw : null;
}
function resetStops() { stopAuto.sl = true; stopAuto.tp = true; }

let sltpEdit = false;
/**
 * @param {'deal'|'pending'} kind
 * @param {any} row
 * @param {'sl'|'tp'} field
 */
function openLevels(kind, row, field) {
  Levels.open({
    kind, row, field, t: wT, num: fmtN,
    ref: () => {
      if (kind === 'pending') return row.price;
      const it = META[row.instrument];
      const raw = it ? (row.side === 'buy' ? it.bid : it.ask) : null;
      return Number.isInteger(raw) && raw > 0 ? raw : null;
    },
    bps: () => minStopBps,
    submit: (/** @type {any} */ body) => (kind === 'deal' ? api.editDealLevels(row.id, body) : api.editPendingLevels(row.id, body)),
    rejection: rejectionText,
    onDone: () => { panel.refresh(); pollAccount(); },
  });
}
function refreshStops() {
  const it = META[selected];
  const hint = document.getElementById('stopsHint');
  const ref = stopRef(it);
  const sug = it && ref != null ? Stops.suggest(orderSide, ref, minStopBps, orderMode === 'market') : null;
  for (const k of /** @type {Array<'sl'|'tp'>} */ (['sl', 'tp'])) {
    if (stopAuto[k]) stopInput(k).value = sug ? (sug[k] / pscale(it.id)).toFixed(pdec(it.id)) : '';
  }
  document.getElementById('stopsReset').hidden = stopAuto.sl && stopAuto.tp;
  if (!it) { hint.textContent = ''; return; }
  if (minStopBps > 0) {
    const d = ref != null ? fmtP(it.id, Stops.minDistance(ref, minStopBps)) : '—';
    hint.textContent = tv('f.stops.min', { p: I18N.num(minStopBps / 100, 2), d });
  } else hint.textContent = tv('f.stops.hint', { p: I18N.num(Stops.HINT_BPS / 100) });
  validateStops();
}
function validateStops() {
  const it = META[selected];
  const ref = stopRef(it);
  const { sl, tp } = it ? readSlTp(it.id) : { sl: null, tp: null };
  const bad = it && ref != null ? Stops.check(orderSide, ref, sl, tp, minStopBps) : null;
  stopInput('sl').classList.toggle('bad', !!bad && bad.code === 'stop_loss_too_close');
  stopInput('tp').classList.toggle('bad', !!bad && bad.code === 'take_profit_too_close');
  return bad;
}
for (const k of /** @type {Array<'sl'|'tp'>} */ (['sl', 'tp'])) {
  stopInput(k).addEventListener('input', () => { stopAuto[k] = false; document.getElementById('stopsReset').hidden = false; validateStops(); });
}
document.getElementById('stopsReset').addEventListener('click', () => { resetStops(); refreshStops(); });
document.getElementById('price').addEventListener('input', () => refreshStops());
document.getElementById('shares').addEventListener('click', (e) => {
  const b = /** @type {HTMLElement} */ (/** @type {Element} */ (e.target).closest('[data-share]'));
  const it = META[selected];
  const a = lastAccount;
  if (!b || !it || !a || !(a.free_margin > 0) || !(a.leverage >= 1)) return;
  const p = priceOrNull(it);
  if (p === null) return;
  const usdAmount = (a.free_margin / 100) * a.leverage * (+b.dataset.share / 100);
  const inp = /** @type {HTMLInputElement} */ (document.getElementById('lot'));
  if (sizeUnit === 'usd') inp.value = usdAmount.toFixed(0);
  else if (Sizing.contract(it)) inp.value = Sizing.toLots(it, Sizing.fromUnits(it, usdAmount / p)).toFixed(Sizing.lotDecimals(it));
  else inp.value = (Math.floor((usdAmount / p) * qscale(it.id)) / qscale(it.id)).toFixed(qdec(it.id));
  updateSizeEq();
});
document.getElementById('lotMinus').addEventListener('click', () => stepLot(-1));
document.getElementById('lotPlus').addEventListener('click', () => stepLot(1));
function stepLot(d) {
  const i = /** @type {HTMLInputElement} */ (document.getElementById('lot'));
  const it = META[selected];
  const cur = parseFloat(i.value) || 0;
  if (sizeUnit === 'usd') i.value = Math.max(1, cur + d * 10).toFixed(0);
  else if (Sizing.contract(it)) {
    const st = Sizing.toLots(it, Sizing.step(it));
    const lo = Sizing.toLots(it, Sizing.minQty(it) ?? Sizing.step(it));
    i.value = Math.max(lo, cur + d * st).toFixed(Sizing.lotDecimals(it));
  } else {
    const st = Math.max(0.01, Sizing.toUnits(it, Sizing.step(it)));
    const lo = Math.max(0.001, Sizing.minQty(it) === null ? 0 : Sizing.toUnits(it, Sizing.minQty(it)));
    i.value = Math.max(lo, cur + d * st).toFixed(st >= 1 ? 0 : 3);
  }
  updateSizeEq();
}
function syncLotStep() {
  const it = META[selected];
  const inp = /** @type {HTMLInputElement} */ (document.getElementById('lot'));
  inp.step = sizeUnit === 'usd' ? '1' : Sizing.contract(it) ? String(Sizing.toLots(it, Sizing.step(it))) : '0.001';
}
function readSlTp(id) { const sl = parseFloat(/** @type {HTMLInputElement} */ (document.getElementById('sl')).value), tp = parseFloat(/** @type {HTMLInputElement} */ (document.getElementById('tp')).value); return { sl: sl > 0 ? Math.round(sl * pscale(id)) : null, tp: tp > 0 ? Math.round(tp * pscale(id)) : null }; }
function sizeBaseQty(id) { const v = parseFloat(/** @type {HTMLInputElement} */ (document.getElementById('lot')).value) || 0; if (v <= 0) return 0; if (sizeUnit === 'usd') { const p = refPrice(META[id]) / pscale(id); return p > 0 ? v / p : 0; } return v * (Sizing.contract(META[id]) || 1); }
/** @param {number} id */
const sizeRaw = (id) => Sizing.fromUnits(META[id], sizeBaseQty(id));
/** @type {{ markup_bps: number, commission_bps: number }} */
let tariff = { markup_bps: 0, commission_bps: 0 };

function updateFeeInfo() {
  const el = document.getElementById('feeInfo');
  if (!el) return;
  const it = META[selected];
  const bps = tariff.commission_bps;
  if (!it || !(bps > 0)) { el.classList.add('hidden'); el.textContent = ''; return; }
  const qty = sizeRaw(it.id);
  let px = null;
  if (orderMode === 'limit') {
    const v = parseFloat(/** @type {HTMLInputElement} */ (document.getElementById('price')).value);
    if (v > 0) px = Math.round(v * pscale(it.id));
  } else if (it.ask != null) {
    px = Tariff.execPrice(true, it.ask, tariff.markup_bps);
  }
  const minor = px == null ? NaN : Tariff.commissionMinor(px, qty, pdec(it.id), qdec(it.id), bps);
  el.textContent = Number.isFinite(minor) ? tv('fee.text', { v: I18N.num(minor / 100, 2) }) : T('fee.unknown');
  el.title = tv('fee.tip', { p: I18N.num(bps / 100, 0, 2) });
  el.classList.remove('hidden');
}

let tradingFrozen = false;
/**
 * @param {any} it
 * @returns {string|null}
 */
function gateBlock(it) {
  if (!it) return null;
  const why = orderMode === 'limit' ? Market.blockPending(it) : Market.blockOpen(it);
  if (!why) return null;
  if (why === 'closeOnly') return T('gate.closeOnly');
  if (why === 'untradable') return T('gate.untradable');
  return `${marketText(it)}. ${T(why === 'closed' ? 'gate.closed' : 'gate.halted')}`;
}
/**
 * @param {any} it
 * @returns {string|null}
 */
function sizeBlock(it) {
  if (!it) return null;
  const v = parseFloat(/** @type {HTMLInputElement} */ (document.getElementById('lot')).value) || 0;
  if (!(v > 0) || (sizeUnit === 'usd' && priceOrNull(it) === null)) return null;
  const min = Sizing.minQty(it) ?? (Sizing.step(it) > 1 ? Sizing.step(it) : null);
  if (min === null || sizeRaw(it.id) >= min) return null;
  return tv('sz.belowMin', { min: sizeLabel(it, min) });
}
function applyGate() {
  const it = META[selected];
  const block = gateBlock(it) || sizeBlock(it);
  const waits = !block && orderMode === 'limit' && it && Market.state(it).status !== 'open' ? T('gate.waits') : '';
  ['btnBuy', 'btnSell'].forEach((id) => {
    const b = /** @type {HTMLButtonElement} */ (document.getElementById(id));
    b.disabled = tradingFrozen || !!block;
    b.title = block || '';
  });
  const el = document.getElementById('dealGate');
  el.hidden = !(block || waits);
  el.textContent = block || waits;
  el.classList.toggle('deal-gate--info', !block);
}
function renderSizeHint() {
  const it = META[selected];
  const el = document.getElementById('sizeHint');
  const min = it ? Sizing.minQty(it) : null;
  const st = it ? Sizing.step(it) : 1;
  if (!it || ((min ?? 1) <= 1 && st <= 1)) { el.hidden = true; el.textContent = ''; return; }
  const parts = [tv('sz.limits', { min: sizeLabel(it, min ?? st), step: sizeLabel(it, st) })];
  const raw = sizeRaw(it.id);
  if (raw > 0 && raw !== Math.round(sizeBaseQty(it.id) * qscale(it.id))) parts.push(tv('sz.rounded', { v: sizeLabel(it, raw) }));
  el.hidden = false;
  el.textContent = parts.join(' · ');
}

function updateSizeEq() {
  updateFeeInfo();
  renderSizeHint();
  applyGate();
  const it = META[selected];
  const el = document.getElementById('sizeEq');
  const mEl = document.getElementById('marginEq');
  if (!it) { el.textContent = ''; el.title = ''; mEl.textContent = ''; return; }

  const p = priceOrNull(it);
  const v = parseFloat(/** @type {HTMLInputElement} */ (document.getElementById('lot')).value) || 0;
  const cs = Sizing.contract(it);
  const base = cs ? T('sz.lot') : pairOf(it)[0];

  if (p === null) {
    el.textContent = sizeUnit === 'usd' ? `≈ — ${base}` : `${T('f.notional')} ≈ $—`;
    el.title = T('f.priceUnknown');
    mEl.textContent = '';
    return;
  }

  el.title = '';
  el.textContent = sizeUnit === 'usd'
    ? `≈ ${cs ? I18N.num(v / p / cs, Sizing.lotDecimals(it)) : I18N.num(v / p, qdec(it.id))} ${base}`
    : `${T('f.notional')} ≈ ${usd(Math.round((cs ? Sizing.toUnits(it, sizeRaw(it.id)) : v) * p * 100))}`;
  const lev = lastAccount && Number.isInteger(lastAccount.leverage) && lastAccount.leverage >= 1 ? lastAccount.leverage : null;
  const notional = sizeUnit === 'usd' ? v : (cs ? Sizing.toUnits(it, sizeRaw(it.id)) : v) * p;
  mEl.textContent = lev && notional > 0 ? `${T('f.marginReq')} ≈ ${usd(Math.round((notional / lev) * 100))}` : '';
}

function priceOrNull(it) {
  const raw = refPrice(it);
  if (raw == null) return null;
  const p = raw / pscale(it.id);
  return Number.isFinite(p) && p > 0 ? p : null;
}
function setUnit(u) {
  if (sizeUnit === u) return;
  const it = META[selected];
  const inp = /** @type {HTMLInputElement} */ (document.getElementById('lot'));
  const v = parseFloat(inp.value) || 0;
  const p = it ? priceOrNull(it) : null;

  const cs = it ? Sizing.contract(it) : null;
  if (p !== null) {
    inp.value = u === 'usd' ? (v * (cs || 1) * p).toFixed(0) : cs ? (v / p / cs).toFixed(Sizing.lotDecimals(it)) : (v / p).toFixed(3);
  }

  sizeUnit = u;
  syncLotStep();
  document.getElementById('unitLot').classList.toggle('active', u === 'lot');
  document.getElementById('unitUsd').classList.toggle('active', u === 'usd');
  updateSizeEq();
}
document.getElementById('unitLot').addEventListener('click', () => setUnit('lot'));
document.getElementById('unitUsd').addEventListener('click', () => setUnit('usd'));
document.getElementById('lot').addEventListener('input', updateSizeEq);
const rejectionText = (/** @type {number} */ status, /** @type {string} */ code) => I18N.rejection(status, code);

async function submit(side) {
  const id = selected; if (id == null) return; const msg = document.getElementById('dealMsg');
  const baseQty = sizeBaseQty(id);
  if (!(baseQty > 0)) { msg.textContent = T('f.enterSize'); msg.style.color = 'var(--down)'; return; }
  if (side !== orderSide) setSide(side);
  refreshStops();
  const bad = validateStops();
  if (bad) {
    msg.textContent = tv(bad.code === 'stop_loss_too_close' ? 'f.sl.bad' : 'f.tp.bad', { px: fmtP(id, bad.limit) });
    msg.style.color = 'var(--down)';
    return;
  }
  const gate = gateBlock(META[id]) || sizeBlock(META[id]);
  if (gate) { msg.textContent = gate; msg.style.color = 'var(--down)'; return; }
  const { sl, tp } = readSlTp(id); const qty = sizeRaw(id); let body;
  if (!(qty > 0)) { msg.textContent = T('f.enterSize'); msg.style.color = 'var(--down)'; return; }
  const isLimit = orderMode === 'limit';
  if (isLimit) { const price = parseFloat(/** @type {HTMLInputElement} */ (document.getElementById('price')).value); if (!(price > 0)) { msg.textContent = T('f.enterPrice'); msg.style.color = 'var(--down)'; return; } body = { instrument: id, side, qty, price: Math.round(price * pscale(id)), sl, tp }; }
  else body = { instrument: id, side, qty, sl, tp };
  const { ok, status, data } = isLimit ? await api.placePending(body) : await api.openDeal(body);
  if (ok) { msg.textContent = isLimit ? tv('f.limitPlaced', { id: data.id }) : tv('f.opened', { id: data.id, side: side.toUpperCase(), px: fmtP(id, data.entry) }); msg.style.color = 'var(--up)'; panel.show(isLimit ? 'pending' : 'deals'); pollAccount(); panel.refresh(); resetStops(); refreshStops(); }
  else {
    const near = Number.isInteger(data?.limit) ? ' ' + tv('f.nearest', { px: fmtP(id, data.limit) }) : '';
    msg.textContent = refusal(status, data) + near; msg.style.color = 'var(--down)';
  }
}
document.getElementById('btnBuy').addEventListener('click', () => submit('buy'));
document.getElementById('btnSell').addEventListener('click', () => submit('sell'));

const ACCT_KEYS = ['no-session', 'unreachable', 'unknown', 'not-opened'];
const acctMsg = (/** @type {string} */ status) => T('acct.' + (ACCT_KEYS.includes(status) ? status : 'unknown'));
function showAcctWarn(status) {
  const el = document.getElementById('acctWarn');
  if (!el) return;
  const msg = status === 'ok' ? null : acctMsg(status);
  el.hidden = !msg;
  el.textContent = msg || '';
}
function showPositionModel(model) {
  const el = document.getElementById('modelBadge');
  if (!el) return;
  if (!model) { el.hidden = true; return; }
  const netting = model === 'netting';
  el.hidden = false;
  el.textContent = netting ? T('model.netting') : T('model.hedging');
  el.title = netting ? T('model.netting.tip') : T('model.hedging.tip');
}

/**
 * @param {string | undefined} kind
 * @param {boolean | undefined} frozen
 */
function showAccountKind(kind, frozen) {
  const el = document.getElementById('kindBadge');
  if (el) {
    const real = kind === 'real';
    el.hidden = !kind;
    el.textContent = real ? T('kind.real') : T('kind.demo');
    el.classList.toggle('kind-badge--real', real);
    el.title = real ? T('kind.real.tip') : T('kind.demo.tip');
  }
  const f = document.getElementById('frozenBadge');
  if (f) {
    f.hidden = !frozen;
    f.textContent = T('frozen.badge');
    f.title = T('frozen.tip');
  }
  const banner = document.getElementById('frozenBanner');
  if (banner) { banner.hidden = !frozen; banner.textContent = T('frozen.banner'); }
  const form = document.getElementById('dealFrozen');
  if (form) form.hidden = !frozen;
  tradingFrozen = !!frozen;
  applyGate();
}

/** @param {any} a */
function showMarginLevel(a) {
  const el = document.getElementById('mLevel');
  const banner = document.getElementById('mcBanner');
  const lvl = a && Number.isFinite(a.margin_level_pct) ? a.margin_level_pct : null;
  const call = a && Number.isFinite(a.margin_call_level_pct) ? a.margin_call_level_pct : null;
  const stop = a && Number.isFinite(a.stop_out_level_pct) ? a.stop_out_level_pct : null;
  el.textContent = lvl == null ? '—' : I18N.pct(lvl, 0, 2);
  el.title = lvl == null ? T('acct.level.none') : T('acct.level.tip');
  const bad = lvl != null && call != null && lvl <= call;
  const near = lvl != null && call != null && !bad && lvl <= call * 1.5;
  el.classList.toggle('lvl-bad', bad || (lvl != null && stop != null && lvl <= stop));
  el.classList.toggle('lvl-warn', near);
  let text = null;
  const n = (/** @type {number} */ v) => I18N.num(v, 0, 2);
  if (lvl != null && stop != null && lvl <= stop) text = tv('mc.stopout', { lvl: n(lvl), stop: n(stop) });
  else if (bad) text = stop != null ? tv('mc.call', { lvl: n(lvl), call: n(call), stop: n(stop) }) : tv('mc.call.nostop', { lvl: n(lvl), call: n(call) });
  banner.hidden = !text;
  banner.textContent = text || '';
}

/** @returns {Array<[string, string, string?]> | null} */
function balanceRows() {
  const a = lastAccount;
  if (!a) return null;
  const pct = (/** @type {any} */ v) => (Number.isFinite(v) ? I18N.pct(v, 0, 2) : '—');
  const bps = (/** @type {any} */ b) => (b > 0 ? I18N.pct(b / 100, 2) : T('b.none'));
  const money = (/** @type {any} */ c) => (Number.isFinite(c) ? usd(c) : '—');
  const sign = (/** @type {any} */ c) => (c > 0 ? 'pos' : c < 0 ? 'neg' : '');
  /** @type {Array<[string, string, string?]>} */
  const out = [
    [T('b.kind'), a.kind === 'real' ? T('kind.real') : T('kind.demo'), a.kind === 'real' ? 'warn' : ''],
    [T('b.status'), a.trading_frozen ? T('b.blocked') : T('b.active'), a.trading_frozen ? 'neg' : 'pos'],
  ];
  if (a.opened !== false) {
    out.push(
      [T('acct.balance'), money(a.balance)],
      [T('acct.equity'), money(a.equity)],
      [T('acct.pnl'), money(a.open_pnl), sign(a.open_pnl)],
      [T('acct.margin'), money(a.used_margin)],
      [T('acct.free'), money(a.free_margin)],
      [T('acct.level'), pct(a.margin_level_pct), document.getElementById('mLevel').classList.contains('lvl-bad') ? 'neg' : ''],
    );
  }
  out.push(
    [T('b.callAt'), pct(a.margin_call_level_pct)],
    [T('b.stopAt'), pct(a.stop_out_level_pct)],
    [T('acct.leverage'), Number.isInteger(a.leverage) && a.leverage >= 1 ? `1:${a.leverage}` : '—'],
    [T('b.model'), a.model === 'netting' ? T('model.netting') : a.model === 'hedging' ? T('model.hedging') : '—'],
    [T('b.nbp'), a.negative_balance_protection ? T('b.on') : T('b.off')],
    [T('b.markup'), bps(a.markup_bps)],
    [T('b.commission'), bps(a.commission_bps)],
  );
  if (a.account_id) out.push([T('b.id'), String(a.account_id)]);
  return out;
}

function showLeverage(lev) {
  const el = document.getElementById('mLev');
  if (!el) return;
  el.textContent = Number.isInteger(lev) && lev >= 1 ? `1:${lev}` : '—';
  document.getElementById('fLev').textContent = el.textContent;
}

function blankMetrics() {
  ['mBalance', 'mEquity', 'mMargin', 'mFree', 'mPnl', 'mLev', 'mLevel', 'fFree', 'fLev'].forEach((id) => {
    const el = document.getElementById(id); if (el) { el.textContent = '—'; el.style.color = ''; }
  });
}
async function pollAccount() {
  try {
    const a = await api.account();
    const st = api.tradingStatus();
    showAcctWarn(st);
    if (!a) {
      if (st === 'no-session' || st === 'unknown') { blankMetrics(); lastAccount = null; showMarginLevel(null); }
      return;
    }
    lastAccount = a;
    showAccountKind(a.kind, a.trading_frozen);
    showLeverage(a.leverage);
    tariff = {
      markup_bps: Number.isInteger(a.markup_bps) ? a.markup_bps : 0,
      commission_bps: Number.isInteger(a.commission_bps) ? a.commission_bps : 0,
    };
    minStopBps = Number.isInteger(a.min_stop_bps) && a.min_stop_bps > 0 ? a.min_stop_bps : 0;
    const edit = a.sltp_edit === true;
    if (edit !== sltpEdit) { sltpEdit = edit; panel.refresh(); }
    if (!edit) Levels.close();
    updateDealPanel();
    if (a.opened === false) {
      blankMetrics();
      showLeverage(a.leverage);
      showPositionModel(null);
      const w = document.getElementById('acctWarn');
      if (w) { w.hidden = false; w.textContent = acctMsg('not-opened'); }
      showMarginLevel(null);
      return;
    }
    showPositionModel(a.model);
    document.getElementById('mBalance').textContent = usd(a.balance);
    document.getElementById('mEquity').textContent = usd(a.equity);
    document.getElementById('mMargin').textContent = usd(a.used_margin);
    document.getElementById('mFree').textContent = usd(a.free_margin);
    document.getElementById('fFree').textContent = usd(a.free_margin);
    showMarginLevel(a);
    const p = document.getElementById('mPnl');
    p.textContent = usd(a.open_pnl);
    p.style.color = a.open_pnl > 0 ? 'var(--up)' : a.open_pnl < 0 ? 'var(--down)' : '';
  } catch (e) {
    console.error('[account] метрики счёта не отрисованы', e);
  }
}
function computeSignals() {
  const it = META[selected];
  if (!it || rawCandles.length < 30) return [];
  const close = rawCandles.map((c) => c.close); const out = [];
  const r = rsi(close, 14), rv = r[r.length - 1];
  if (rv != null && rv > 70) out.push(['RSI overbought', 'SELL']);
  else if (rv != null && rv < 30) out.push(['RSI oversold', 'BUY']);
  const f = sma(close, 9), sl = sma(close, 21), i = close.length - 1;
  if (f[i] != null && sl[i] != null && f[i - 1] != null && sl[i - 1] != null) {
    if (f[i - 1] <= sl[i - 1] && f[i] > sl[i]) out.push(['MA 9/21 cross up', 'BUY']);
    else if (f[i - 1] >= sl[i - 1] && f[i] < sl[i]) out.push(['MA 9/21 cross down', 'SELL']);
  }
  return out.map(([name, dir]) => ({ symbol: it.symbol, name, tf: TFN[tf], dir }));
}

function applyChartTheme() {
  chart.applyOptions({
    layout: { background: { color: rgbT('--bg-base') }, textColor: rgbT('--text-secondary') },
    grid: { vertLines: { color: rgbaT('--text-secondary', '.10') }, horzLines: { color: rgbaT('--text-secondary', '.10') } },
    rightPriceScale: { borderColor: rgbaT('--text-secondary', '.25') },
    timeScale: { borderColor: rgbaT('--text-secondary', '.25') },
  });
  volume.applyOptions({ color: rgbaT('--text-secondary', '.30') });
  if (selected != null && rawCandles.length) {
    makeMainSeries();
    mainSeries.setData(mainData());
    applyIndicators();
    redrawPriceLines();
    volume.setData(rawCandles.map(volPoint));
  }
}
document.addEventListener('themechange', applyChartTheme);

function renderLangButtons() {
  const box = /** @type {HTMLElement} */ (document.querySelector('.lang'));
  /** @type {string[]} */
  const langs = I18N.available();
  const want = langs.join(',');
  if (box.dataset.built !== want) {
    box.dataset.built = want;
    box.innerHTML = langs.map((l) => `<button type="button" data-lang="${l}" lang="${l}" title="${I18N.NAMES[l]}">${l.toUpperCase()}</button>`).join('');
  }
  box.style.display = langs.length > 1 ? '' : 'none';
  box.querySelectorAll('[data-lang]').forEach((b) => b.classList.toggle('active', /** @type {HTMLElement} */ (b).dataset.lang === I18N.lang()));
}
document.querySelector('.lang').addEventListener('click', (e) => {
  const b = /** @type {HTMLElement} */ (/** @type {Element} */ (e.target).closest('[data-lang]'));
  if (b) I18N.setLang(b.dataset.lang);
});
I18N.onChange(() => {
  renderLangButtons();
  chart.applyOptions({ localization: { locale: I18N.locale() } });
  updateDealPanel(); renderFeedStatus(); refreshLegend(); renderMarketWarn(); showStorageMode(); syncFullBtn(); syncTimeframes(META[selected]); if (META[selected]?.has_trades === false) trades.clearTape();
  pollAccount(); panel.refresh(); panel.refreshSignals(); trades.refreshMine(); book.refresh(); stats.refresh(); watchlist.refresh(); renderSsoUi();
});

function setMTab(/** @type {string} */ m) {
  document.body.dataset.mtab = m;
  document.querySelectorAll('#mtabs [data-m]').forEach((b) => b.classList.toggle('active', /** @type {HTMLElement} */ (b).dataset.m === m));
}
document.getElementById('mtabs').addEventListener('click', (e) => {
  const b = /** @type {HTMLElement} */ (/** @type {Element} */ (e.target).closest('[data-m]'));
  if (b) setMTab(b.dataset.m);
});
const phone = window.matchMedia('(max-width: 860px)');
document.getElementById('ihPair').addEventListener('click', () => {
  if (phone.matches) { document.body.classList.toggle('m-watch'); return; }
  const q = /** @type {HTMLInputElement|null} */ (document.querySelector('#pairs input'));
  if (q) { q.focus(); q.select(); }
});
document.getElementById('watchClose').addEventListener('click', () => document.body.classList.remove('m-watch'));
document.getElementById('tapeTrack').addEventListener('click', (e) => {
  const it = /** @type {HTMLElement} */ (/** @type {Element} */ (e.target).closest('[data-id]'));
  if (it) selectInstrument(+it.dataset.id);
});

function renderSsoUi() {
  const S = window.Sso;
  const top = S.mode() === 'top';
  const cab = S.cabinetUrl();
  const link = /** @type {HTMLAnchorElement} */ (document.getElementById('toCabinet'));
  link.hidden = !(top && cab);
  if (cab) link.href = cab;
  document.getElementById('logoutBtn').hidden = !(top && S.embedded() && !!S.token());
  let text = null, btn = null;
  if (top && S.embedded()) {
    const rem = S.remainingMs();
    const st = S.state();
    if (S.token() && rem > 0 && rem < 2 * 60 * 1000) { text = tv('sso.expiring', { min: Math.max(1, Math.ceil(rem / 60000)) }); btn = 'sso.extend'; }
    else if (!S.token()) { text = st === 'failed' ? T('sso.failed') : st === 'expired' ? T('sso.expired') : T('sso.none'); btn = cab ? 'sso.login' : null; }
  }
  const banner = document.getElementById('ssoBanner');
  banner.hidden = !text;
  document.getElementById('ssoBannerText').textContent = text || '';
  const b = document.getElementById('ssoBannerBtn');
  b.hidden = !btn;
  if (btn) b.textContent = T(btn);
}
document.getElementById('ssoBannerBtn').addEventListener('click', () => window.Sso.relaunch(true));
document.getElementById('logoutBtn').addEventListener('click', () => window.Sso.logout({ redirect: true }));

I18N.apply();
renderLangButtons();
renderIcons();
applyLayout();
initResizers();
setMode('market');
setSide('buy');
document.querySelectorAll('#indMenu [data-ov]').forEach((x) => x.classList.toggle('on', !!overlays[/** @type {HTMLElement} */ (x).dataset.ov]));
document.getElementById('themeToggle')?.addEventListener('click', () => window.Theming.toggleTheme());
window.Theming.init();
window.Sso.init().onChange(() => { pollAccount(); renderSsoUi(); });
renderSsoUi();
setInterval(renderSsoUi, 1000);
showStorageMode();
setInterval(syncLast, 2000);
setInterval(pollAccount, 1000);
connectWS();
pollAccount();
