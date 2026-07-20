'use strict';

// ============================ State ========================================
let META = {}, selected = null, tf = 3600, orderMode = 'market';
let chartType = 'candles', priceType = 'mid', showPips = false, showSLTP = true, sizeUnit = 'lot';
const overlays = { sma: 0, ema: 0, wma: 0, supertrend: 0, psar: 0, ichimoku: 0, alligator: 0, boll: 0, keltner: 0, donchian: 0, vwap: 0 };
let oscillator = '';
let rawCandles = [], currentDeals = [];

const token = () => document.getElementById('userSel').value;
// Единственная точка доступа к REST (ADR-015): заголовки и разбор ответов — в api.js.
const api = Api.create({ token });
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
  caret: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.6"><path d="M4 6l4 4 4-4"/></svg>',
  reset: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><path d="M12.5 8a4.5 4.5 0 1 1-1.3-3.2"/><path d="M12.5 2.5V5H10"/></svg>',
  fullscreen: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.4"><path d="M2 6V2h4M14 6V2h-4M2 10v4h4M14 10v4h-4"/></svg>',
  settings: '<svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.25"><circle cx="8" cy="8" r="2.1"/><path d="M8 1.6v1.8M8 12.6v1.8M1.6 8h1.8M12.6 8h1.8M3.5 3.5l1.3 1.3M11.2 11.2l1.3 1.3M12.5 3.5l-1.3 1.3M4.8 11.2l-1.3 1.3"/></svg>',
};
function renderIcons(root = document) { root.querySelectorAll('[data-icon]').forEach((el) => { const n = el.dataset.icon; if (ICONS[n]) el.innerHTML = ICONS[n]; }); }

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
  // Локаль задаём явно: иначе библиотека берёт navigator.language, и подписи оси времени
  // зависят от настроек ОС клиента (а в окружении без локали — падают с RangeError). BUG-008.
  localization: { locale: 'en-US' },
  grid: { vertLines: { color: 'rgba(31,39,53,.4)' }, horzLines: { color: 'rgba(31,39,53,.4)' } },
  rightPriceScale: { borderColor: '#1f2735' },
  timeScale: { borderColor: '#1f2735', timeVisible: true, secondsVisible: false },
  crosshair: { mode: LightweightCharts.CrosshairMode.Normal },
});
const volume = chart.addHistogramSeries({ priceFormat: { type: 'volume' }, priceScaleId: '', color: '#2b3648' });
volume.priceScale().applyOptions({ scaleMargins: { top: 0.85, bottom: 0 } });
// Хуки для модуля рисования (drawings.js подключается после app.js).
// Инструменты рисования — виджет по ADR-015: сам создаёт canvas и панель внутри .chartwrap.
const draw = Drawings.mount(chartEl.parentElement, { chart, series: () => mainSeries });

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
    const raw = (await api.candles(id, timeframe, 300)) || [];
    const s = pscale(id), v = qscale(id);
    rawCandles = raw.map((c) => ({ time: c.time, open: c.open / s, high: c.high / s, low: c.low / s, close: c.close / s, volume: c.volume / v }));
    makeMainSeries();
    mainSeries.setData(mainData());
    volume.setData(rawCandles.map((c) => ({ time: c.time, value: c.volume, color: volColor(c) })));
    applyIndicators(); redrawPriceLines(); panel.refreshSignals();
    if (rawCandles.length) { const last = raw[raw.length - 1]; lastBarTime = last.time; formingRaw = { ...last }; const n = rawCandles.length; chart.timeScale().setVisibleLogicalRange({ from: Math.max(0, n - 90), to: n + 3 }); } else { lastBarTime = 0; formingRaw = null; }
    refreshLegend();
    draw.clear(); // logical-якоря объектов рисования после setData невалидны
  } finally { load.classList.add('hidden'); }
}
function drawForming() { if (!formingRaw || !mainSeries || chartType === 'heikin') return; const s = pscale(selected); mainSeries.update(isLineType() ? { time: formingRaw.time, value: formingRaw.close / s } : { time: formingRaw.time, open: formingRaw.open / s, high: formingRaw.high / s, low: formingRaw.low / s, close: formingRaw.close / s }); refreshLegend(); }
function onLiveTrade(id, price, qty) { if (id !== selected) return; const now = Math.floor(Date.now() / 1000); const b = now - (now % tf); if (!formingRaw || b > formingRaw.time) formingRaw = { time: b, open: price, high: price, low: price, close: price, volume: qty }; else { formingRaw.high = Math.max(formingRaw.high, price); formingRaw.low = Math.min(formingRaw.low, price); formingRaw.close = price; } if (formingRaw.time >= lastBarTime) { lastBarTime = formingRaw.time; drawForming(); } }
async function syncLast() { const id = selected; if (id == null) return; const raw = (await api.candles(id, tf, 2)) || []; if (!raw.length) return; const last = raw[raw.length - 1]; if (last.time >= lastBarTime) { lastBarTime = last.time; formingRaw = { ...last }; drawForming(); } }

// ============================ Instruments ==================================
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

// ---- Виджеты (ADR-015): страница даёт контейнер, зависимости идут через ctx --
const book = OrderBook.mount(document.getElementById('book'), {
  instrument: () => selected,
  fetch: (id, limit) => api.depth(id, limit),
  onPick: (price) => { setMode('limit'); const inp = document.getElementById('price'); inp.value = price; inp.classList.add('flash'); setTimeout(() => inp.classList.remove('flash'), 400); },
  // Подписи колонок и марк-цена — из данных инструмента, которыми владеет хост.
  symbols: (id) => { const [base, quote] = (META[id]?.symbol || '-').split('-'); return { base, quote }; },
  markPrice: (id) => (META[id]?.last != null ? META[id].last / pscale(id) : null),
  interval: 700,
  storagePrefix: 'ob',
});
const stats = AssetStats.mount(document.getElementById('statbar'), {
  instrument: () => selected,
  fetch: (id) => api.stats(id),
  labels: TFN,
  interval: 5000,
});
const panel = Positions.mount(document.getElementById('bottom'), {
  api,
  fmtUsd: usd,
  pips: () => showPips,
  // Позиции нужны графику для линий SL/TP — виджет отдаёт их хосту, а не лезет в график сам.
  onDeals: (list) => { currentDeals = list.filter((d) => d.instrument === selected); redrawPriceLines(); },
  onChange: () => pollAccount(),
  signals: () => computeSignals(),
  interval: 1500,
});
const trades = Trades.mount(document.getElementById('trades'), {
  instrument: () => selected,
  decimals: (id) => ({ pd: pdec(id), qd: qdec(id) }),
  // «Мои сделки» — по выбранному инструменту; общая история живёт в нижней панели.
  myTrades: async (id) => ((await api.closedDeals()) || []).filter((d) => d.instrument === id),
  fmtUsd: usd,
  interval: 2500,
});
// Монтируется последним: его первый refresh выбирает инструмент, а selectInstrument
// обращается к book/stats/panel/trades — они должны уже существовать.
const watchlist = Watchlist.mount(document.querySelector('.watch'), {
  fetch: () => api.instruments(),
  instrument: () => selected,
  onData: (list) => { list.forEach((it) => (META[it.id] = it)); updateDealPanel(); },
  onSelect: (id) => selectInstrument(id),
  interval: 1000,
});
async function selectInstrument(id) { selected = id; watchlist.setActive(id); document.getElementById('price').value = ''; document.getElementById('sl').value = ''; document.getElementById('tp').value = ''; await loadCandles(id, tf); updateDealPanel(); pollAccount(); panel.refreshDeals(); book.reset(); book.refresh(); stats.refresh(); trades.clearTape(); trades.refreshMine(); }
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
document.getElementById('pipsBtn').addEventListener('click', () => { showPips = !showPips; document.getElementById('pipsBtn').classList.toggle('on', showPips); updateDealPanel(); panel.refreshDeals(); });

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

// Видимость колонок списка: состояние и отрисовка — внутри виджета, страница лишь
// связывает свои чекбоксы с его публичным API.
function bindCol(id, key) { const el = document.getElementById(id); if (!el) return; el.checked = !!watchlist.columns()[key]; el.addEventListener('change', () => watchlist.setColumns({ [key]: el.checked ? 1 : 0 })); }
bindCol('cChange', 'change'); bindCol('cSell', 'sell'); bindCol('cBuy', 'buy'); bindCol('cSpread', 'spread'); bindCol('cHigh', 'high');

// Panel/toolbar resizing
// leftW=440: при 280px шесть колонок списка не помещались и цены резались многоточием (BUG-009).
// Замер по всем 30 строкам: 360px → 30 обрезанных ячеек, 400 → 6, 440 → 3, полный ноль только
// при 520px. 520 отдаёт списку 40% экрана и душит график, поэтому берём 440 как баланс.
// Сохранённый пользователем размер (localStorage) имеет приоритет над этим дефолтом.
const layout = Object.assign({ leftW: 300, rightW: 340, bottomH: 190, toolbarH: 42, dealH: 150, tradesH: 240 }, JSON.parse(localStorage.getItem('layout') || '{}'));
function applyLayout() {
  document.querySelector('.grid').style.gridTemplateColumns = `${layout.leftW}px 6px 1fr 6px ${layout.rightW}px`;
  document.querySelector('.bottom').style.height = layout.bottomH + 'px';
  document.querySelector('.deal').style.height = layout.dealH + 'px';
  document.getElementById('trades').style.height = layout.tradesH + 'px';
  // minHeight, а не height: тулбар должен иметь право вырасти, когда кнопки не влезли
  // в одну строку. Жёсткая высота выталкивала их поверх панели метрик (BUG-010).
  document.querySelector('.toolbar').style.minHeight = layout.toolbarH + 'px';
}
function initResizers() {
  document.querySelectorAll('[data-rsz]').forEach((h) => h.addEventListener('mousedown', (e) => {
    e.preventDefault(); h.classList.add('drag');
    const type = h.dataset.rsz, sx = e.clientX, sy = e.clientY, sl = layout.leftW, sr = layout.rightW, sb = layout.bottomH, stH = layout.toolbarH, sd = layout.dealH, str = layout.tradesH;
    const mv = (ev) => {
      if (type === 'left') layout.leftW = Math.min(520, Math.max(180, sl + (ev.clientX - sx)));
      else if (type === 'right') layout.rightW = Math.min(560, Math.max(220, sr - (ev.clientX - sx)));
      else if (type === 'bottom') layout.bottomH = Math.min(window.innerHeight - 320, Math.max(80, sb - (ev.clientY - sy)));
      else if (type === 'deal') layout.dealH = Math.min(340, Math.max(90, sd - (ev.clientY - sy)));
      else if (type === 'trades') layout.tradesH = Math.min(window.innerHeight - 260, Math.max(90, str - (ev.clientY - sy)));
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
function connectWS() { const proto = location.protocol === 'https:' ? 'wss' : 'ws'; const ws = new WebSocket(`${proto}://${location.host}/stream`); const st = document.getElementById('status'); ws.onopen = () => { st.className = 'status status--on'; }; ws.onclose = () => { st.className = 'status status--off'; setTimeout(connectWS, 1500); }; ws.onmessage = (ev) => { let e; try { e = JSON.parse(ev.data); } catch { return; } for (const x of e) if (x.type === 'trade') { onLiveTrade(x.instrument, x.price, x.qty); trades.pushMarket(x); } }; }

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
  const { sl, tp } = readSlTp(id); const qty = Math.round(baseQty * qscale(id)); let body;
  const isLimit = orderMode === 'limit';
  if (isLimit) { const price = parseFloat(document.getElementById('price').value); if (!(price > 0)) { msg.textContent = 'Enter price'; msg.style.color = 'var(--down)'; return; } body = { instrument: id, side, qty, price: Math.round(price * pscale(id)), sl, tp }; }
  else body = { instrument: id, side, qty, sl, tp };
  const { ok, status, data } = isLimit ? await api.placePending(body) : await api.openDeal(body);
  if (ok) { msg.textContent = isLimit ? `Limit order #${data.id}` : `Opened #${data.id}: ${side.toUpperCase()} @ ${fmtP(id, data.entry)}`; msg.style.color = 'var(--up)'; panel.show(isLimit ? 'pending' : 'deals'); pollAccount(); panel.refresh(); }
  else { msg.textContent = `Rejected ${status}: ${data?.error || ''}`; msg.style.color = 'var(--down)'; }
}
document.getElementById('btnBuy').addEventListener('click', () => submit('buy'));
document.getElementById('btnSell').addEventListener('click', () => submit('sell'));
document.getElementById('userSel').addEventListener('change', () => { pollAccount(); panel.refresh(); });

// ============================ Account / positions / history ================
async function pollAccount() { try { const a = await api.account(); if (!a) return; document.getElementById('mBalance').textContent = usd(a.balance); document.getElementById('mEquity').textContent = usd(a.equity); document.getElementById('mMargin').textContent = usd(a.used_margin); document.getElementById('mFree').textContent = usd(a.free_margin); const p = document.getElementById('mPnl'); p.textContent = usd(a.open_pnl); p.style.color = a.open_pnl > 0 ? 'var(--up)' : a.open_pnl < 0 ? 'var(--down)' : ''; } catch (e) { /* */ } }
// Сигналы считаются из свечей графика (данные хоста); показывает их виджет positions.js.
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
// Поиск, сортировка, избранное и клик по строке — внутри watchlist.js (ADR-015).

// ============================ Start ========================================
renderIcons();
applyLayout();
initResizers();
setInterval(syncLast, 2000);
setInterval(pollAccount, 1000);
connectWS();
pollAccount();
