'use strict';

// ============================ State ========================================
let META = {}, selected = null, tf = 3600, orderMode = 'market';
let chartType = 'candles', priceType = 'mid', showPips = false, showSLTP = true, sizeUnit = 'lot';
const overlays = { sma: 0, ema: 0, wma: 0, supertrend: 0, psar: 0, ichimoku: 0, alligator: 0, boll: 0, keltner: 0, donchian: 0, vwap: 0 };
let oscillator = '';
let rawCandles = [], currentDeals = [];

// Единственная точка доступа к REST (ADR-015). Терминал автономный: без логина,
// один демо-счёт по умолчанию (пивот 2026-07-25).
const api = Api.create();
const pdec = (id) => (META[id]?.price_decimals ?? 2);
const qdec = (id) => (META[id]?.qty_decimals ?? 3);
const pscale = (id) => 10 ** pdec(id);
const qscale = (id) => 10 ** qdec(id);

// Цвета графика берём из CSS-токенов платформы (theming-pack, ADR-023), чтобы график
// следовал теме и бренду. Токены — RGB-каналы ("212 164 55"), см. tokens.v1.css.
// Lightweight Charts не понимает современный синтаксис rgb(r g b / a) — собираем
// легаси rgb()/rgba() с запятыми из каналов.
const cssChan = (n) => getComputedStyle(document.documentElement).getPropertyValue(n).trim();
const rgbT = (n) => { const [r, g, b] = cssChan(n).split(/\s+/); return `rgb(${r}, ${g}, ${b})`; };
const rgbaT = (n, a) => { const [r, g, b] = cssChan(n).split(/\s+/); return `rgba(${r}, ${g}, ${b}, ${a})`; };
const fmtP = (id, raw) => (raw / pscale(id)).toLocaleString('en-US', { minimumFractionDigits: pdec(id), maximumFractionDigits: pdec(id) });
const fmtQ = (id, raw) => (raw / qscale(id)).toFixed(qdec(id));
// Знак перед валютой: «$-0.06» читается как опечатка, правильно «-$0.06».
const usd = (c) => (c < 0 ? '-' : '') + '$' + (Math.abs(c) / 100).toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
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
function renderIcons(root = document) { root.querySelectorAll('[data-icon]').forEach((el) => { const n = /** @type {HTMLElement} */ (el).dataset.icon; if (ICONS[n]) el.innerHTML = ICONS[n]; }); }

// ---- Legend (chart info card) ---------------------------------------------
function fmtDT(t) { return new Date(t * 1000).toLocaleString('en-GB', { timeZone: 'UTC', day: '2-digit', month: 'short', year: 'numeric', hour: '2-digit', minute: '2-digit', hour12: false }) + ' UTC'; }
/**
 * @param {number} id @param {number} t @param {any} c
 * @param {boolean} [forming] бар ещё не закрыт — и обязан сказать это сам.
 *
 * Это условие решения по TRM-33, а не украшение. Гистограмма объёма теперь
 * живая, а индикаторы считаются по закрытым барам; без пометки расхождение
 * «индикатор отстаёт на один бар» читалось бы как поломка — ровно та
 * необъяснимость, которую мы только что нашли у объёма.
 */
function updateLegend(id, t, c, forming) {
  const rf = (x) => x.toLocaleString('en-US', { minimumFractionDigits: pdec(id), maximumFractionDigits: pdec(id) });
  document.getElementById('lgSym').textContent = META[id]?.symbol || '';
  document.getElementById('lgTf').textContent = TFN[tf] || '';
  document.getElementById('lgTime').textContent = fmtDT(t) + (forming ? ' · бар не закрыт' : '');
  document.getElementById('lgOhlc').innerHTML = `O <b>${rf(c.open)}</b>  H <b>${rf(c.high)}</b>  L <b>${rf(c.low)}</b>  C <b class="${c.close >= c.open ? 'up' : 'down'}">${rf(c.close)}</b>`;
}
/**
 * Формирующийся бар В ТОМ ВИДЕ, В КАКОМ ОН НАРИСОВАН (TRM-34).
 *
 * Хейкин-аши — не настройка показа, а ДРУГОЙ РАСЧЁТ: его свеча выводится из
 * всей цепочки предыдущих. Поэтому формирующийся бар нельзя просто поделить
 * на масштаб и отдать — его надо пересчитать тем же способом, каким посчитан
 * остальной ряд, иначе последняя свеча жила бы по другим правилам, чем те,
 * рядом с которыми она стоит.
 *
 * @param {any} b формирующийся бар в raw-единицах
 */
function formingDisplayBar(b) {
  const s = pscale(selected), v = qscale(selected);
  const real = { time: b.time, open: b.open / s, high: b.high / s, low: b.low / s, close: b.close / s, volume: b.volume / v };
  if (chartType !== 'heikin') return real;
  // Цепочку берём ту же, что у нарисованного ряда. Последний бар REST — это и
  // есть формирующийся, поэтому он ЗАМЕЩАЕТСЯ, а не добавляется вторым.
  const n = rawCandles.length;
  const src = n && rawCandles[n - 1].time === real.time ? rawCandles.slice(0, n - 1).concat([real]) : rawCandles.concat([real]);
  const ha = heikin(src);
  return ha[ha.length - 1];
}
function refreshLegend() { if (selected == null) return; let c, forming = false; const f = priceSeries.forming(); if (f) { c = formingDisplayBar(f); forming = true; } else if (rawCandles.length) { const n = rawCandles.length; c = chartType === 'heikin' ? heikin(rawCandles)[n - 1] : rawCandles[n - 1]; } if (c) updateLegend(selected, c.time, c, forming); }

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
  layout: { background: { color: rgbT('--bg-base') }, textColor: rgbT('--text-secondary'), attributionLogo: false },
  // Локаль задаём явно: иначе библиотека берёт navigator.language, и подписи оси времени
  // зависят от настроек ОС клиента (а в окружении без локали — падают с RangeError). BUG-008.
  localization: { locale: 'en-US' },
  grid: { vertLines: { color: rgbaT('--text-secondary', '.10') }, horzLines: { color: rgbaT('--text-secondary', '.10') } },
  rightPriceScale: { borderColor: rgbaT('--text-secondary', '.25') },
  timeScale: { borderColor: rgbaT('--text-secondary', '.25'), timeVisible: true, secondsVisible: false },
  crosshair: { mode: LightweightCharts.CrosshairMode.Normal },
});
const volume = chart.addHistogramSeries({ priceFormat: { type: 'volume' }, priceScaleId: '', color: rgbaT('--text-secondary', '.30') });
volume.priceScale().applyOptions({ scaleMargins: { top: 0.85, bottom: 0 } });
// Хуки для модуля рисования (drawings.js подключается после app.js).
// Инструменты рисования — виджет по ADR-015: сам создаёт canvas и панель внутри .chartwrap.
const draw = Drawings.mount(chartEl.parentElement, { chart, series: () => mainSeries });

let mainSeries = null, overlaySeries = [], oscSeriesList = [], priceLines = [];
// Формирующаяся свеча и её РАЗРЫВ живут в `series.js` (TRM-19, Р-073). Здесь
// лежала пара модульных переменных, которую правили из четырёх мест, — и
// именно поэтому бар переживал обрыв связи: сбрасывать его было некому.
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
      // Сервер не смог отдать свечи (503) — оставляем прежний график и говорим об
      // этом. Пустой массив стёр бы график, а пустой график читается как «рынка
      // нет», хотя означает «мы не получили данные» (BUG-011).
      load.classList.add('hidden');
      return;
    }
    const raw = fetched;
    const s = pscale(id), v = qscale(id);
    rawCandles = raw.map((c) => ({ time: c.time, open: c.open / s, high: c.high / s, low: c.low / s, close: c.close / s, volume: c.volume / v }));
    makeMainSeries();
    mainSeries.setData(mainData());
    volume.setData(rawCandles.map((c) => ({ time: c.time, value: c.volume, color: volColor(c) })));
    applyIndicators(); redrawPriceLines(); panel.refreshSignals();
    // Загрузка с сервера — авторитетный пересев ряда: ответ REST видел и дыру
    // тоже, поэтому он же снимает разрыв.
    priceSeries.seed(rawCandles.length ? raw[raw.length - 1] : null);
    if (rawCandles.length) { const n = rawCandles.length; chart.timeScale().setVisibleLogicalRange({ from: Math.max(0, n - 90), to: n + 3 }); }
    refreshLegend();
    draw.clear(); // logical-якоря объектов рисования после setData невалидны
  } finally { load.classList.add('hidden'); }
}
/**
 * @param {any} [bar] свеча к отрисовке; без неё — текущая формирующаяся.
 *
 * Объём обновляется ЖИВЬЁМ (TRM-33). Раньше серия объёма менялась только
 * полной загрузкой: цена рисовала новые свечи, а столбцов под ними не
 * появлялось — гистограмма обрывалась на моменте выбора инструмента, и ничто
 * на экране об этом не говорило.
 *
 * ИНДИКАТОРЫ ЖИВЫМИ НЕ СТАНОВЯТСЯ, и это решение, а не недоделка. Гистограмма
 * показывает НАБЛЮДЁННОЕ: сделки произошли, не показать их — скрыть факт.
 * Индикатор показывает ПРОИЗВОДНУЮ величину, и на незакрытом баре она
 * перерисовывается на каждом тике: человек решал бы по линии, которой через
 * секунду не будет. Поэтому они считаются по `rawCandles`, то есть по
 * закрытым барам, и дозакрывать это ради единообразия не надо.
 *
 * Плата за решение — расхождение на один бар, и оно названо вслух пометкой
 * «бар не закрыт» в легенде (см. `updateLegend`).
 */
function drawForming(bar) {
  const b = bar || priceSeries.forming();
  if (!b) return;
  // Объём от типа графика не зависит: хейкин-аши пересчитывает цену, а сделки
  // остаются теми же.
  volume.update({ time: b.time, value: b.volume / qscale(selected), color: volColor(b) });
  if (!mainSeries) return;
  // Здесь стоял выход для хейкин-аши (TRM-34): в этом режиме график не
  // обновлялся живьём ВООБЩЕ — ни свеча, ни легенда, — и пометка «бар не
  // закрыт» не появлялась никогда. Условие решения выполнялось не во всех
  // режимах, то есть не выполнялось. Режим переключает пользователь, и это
  // отдельный путь данных, а не настройка показа.
  const d = formingDisplayBar(b);
  mainSeries.update(isLineType() ? { time: d.time, value: d.close } : { time: d.time, open: d.open, high: d.high, low: d.low, close: d.close });
  refreshLegend();
}
// Агрегация живой сделки в свечу — в `series.js`. Отрисовка остаётся здесь:
// пока ряд разорван, модуль не возвращает ничего, и рисовать нечего.
function onLiveTrade(id, price, qty) { if (id !== selected) return; const b = priceSeries.onTrade({ now: Math.floor(Date.now() / 1000), tf, price, qty }); if (b) drawForming(b); }
async function syncLast() {
  const id = selected; if (id == null) return;
  // Разорванный ряд возвращается ПОЛНОЙ перезагрузкой, а не заплаткой на два
  // бара. Заплатка здесь опасна: после разрыва авторитетный бар может
  // оказаться СТАРШЕ уже нарисованного (наши часы открыли новый интервал, а
  // биржа ещё нет), а график назад не обновляется — он на это падает.
  // Перезагрузка кладёт ряд целиком и такого порядка не знает.
  if (priceSeries.isBroken()) { await loadCandles(id, tf); return; }
  const raw = (await api.candles(id, tf, 2)) || []; if (!raw.length) return;
  const last = raw[raw.length - 1];
  if (last.time >= priceSeries.lastBarTime()) { priceSeries.seed(last); drawForming(); }
}

// ============================ Instruments ==================================
function refPrice(it) { if (!it || it.bid == null || it.ask == null) return it?.last; if (priceType === 'ask') return it.ask; if (priceType === 'bid') return it.bid; return Math.round((it.bid + it.ask) / 2); }
function updateDealPanel() {
  const it = META[selected]; if (!it) return;
  document.getElementById('dealSym').textContent = it.symbol;
  const ref = refPrice(it);
  document.getElementById('dealSub').textContent = `${priceType.toUpperCase()} ${ref != null ? fmtP(it.id, ref) : '—'}`;
  // `change` обнуляемо, как и остальные поля котировки. Здесь стоял
  // незащищённый `it.change.toFixed(2)` — и когда шлюз начал честно отдавать
  // null, панель сделки **падала целиком**, обрывая `selectInstrument`.
  // Это второй потребитель поля; первый (витрина инструментов) был поправлен
  // сразу, а этот пропущен — правку границы данных надо проводить по ВСЕМ
  // потребителям, а не по тому, который попался первым.
  const chg = document.getElementById('dealChange');
  const chgKnown = it.change != null;
  const chgUp = chgKnown && it.change >= 0;
  chg.textContent = chgKnown ? `${chgUp ? '+' : ''}${it.change.toFixed(2)}%` : '—';
  chg.className = `deal__change${chgKnown ? (chgUp ? ' up' : ' down') : ''}`;
  document.getElementById('askPx').textContent = it.ask != null ? fmtP(it.id, it.ask) : '—';
  document.getElementById('bidPx').textContent = it.bid != null ? fmtP(it.id, it.bid) : '—';
  document.getElementById('watermark').textContent = it.symbol;
  const pips = document.getElementById('pipsInfo');
  if (showPips && it.bid != null && it.ask != null) { pips.classList.remove('hidden'); pips.textContent = `Spread: ${it.ask - it.bid} pips`; } else pips.classList.add('hidden');
  if (orderMode === 'limit' && !/** @type {HTMLInputElement} */ (document.getElementById('price')).value && ref != null) /** @type {HTMLInputElement} */ (document.getElementById('price')).value = (ref / pscale(it.id)).toFixed(pdec(it.id));
  updateSizeEq();
}

// ---- Виджеты (ADR-015): страница даёт контейнер, зависимости идут через ctx --
// Режим хранилища объявляется в интерфейсе, а не только в логе gateway. Пометка
// постоянная: закрыть её нельзя, потому что забытая пометка — это снова тишина.
// Проверяется один раз на старте: режим не меняется без перезапуска сервиса.
async function showStorageMode() {
  const h = await api.health();
  const el = document.getElementById('volatileWarn');
  if (!el || !h) return; // health недоступен — не выдумываем режим, молчим
  el.hidden = h.durable !== false;
  el.textContent = 'DEMO STATE — NOT SAVED BETWEEN RESTARTS';
  el.title = `storage: ${h.storage}`;
}

// Рыночные данные: отказ показываем отказом. Бейдж гаснет, когда все источники
// снова отвечают (BUG-011).
const marketDown = { candles: false, depth: false, stats: false, instruments: false };
function markMarketData(kind, ok) {
  if (marketDown[kind] === !ok) return; // ничего не изменилось — не дёргаем DOM
  marketDown[kind] = !ok;
  const el = document.getElementById('dataWarn');
  if (!el) return;
  const down = Object.keys(marketDown).filter((k) => marketDown[k]);
  el.hidden = down.length === 0;
  el.textContent = down.length ? `MARKET DATA UNAVAILABLE: ${down.join(', ').toUpperCase()}` : '';
}

const book = OrderBook.mount(document.getElementById('book'), {
  instrument: () => selected,
  fetch: async (id, limit) => { const r = await api.depthDetailed(id, limit); markMarketData('depth', r.reason === null); return r; },
  onPick: (price) => { setMode('limit'); const inp = /** @type {HTMLInputElement} */ (document.getElementById('price')); inp.value = price; inp.classList.add('flash'); setTimeout(() => inp.classList.remove('flash'), 400); },
  // Подписи колонок и марк-цена — из данных инструмента, которыми владеет хост.
  symbols: (id) => { const [base, quote] = (META[id]?.symbol || '-').split('-'); return { base, quote }; },
  markPrice: (id) => (META[id]?.last != null ? META[id].last / pscale(id) : null),
  interval: 700,
  storagePrefix: 'ob',
});
const stats = AssetStats.mount(document.getElementById('statbar'), {
  instrument: () => selected,
  fetch: async (id) => { const r = await api.statsDetailed(id); markMarketData('stats', r.reason === null); return r; },
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
  // `null` пропускаем как есть: виджет обязан отличить «не прочитали» от
  // «сделок нет». Прежнее `|| []` превращало отказ в утверждение о счёте.
  myTrades: async (id) => {
    const all = await api.closedDeals();
    return all === null ? null : all.filter((d) => d.instrument === id);
  },
  fmtUsd: usd,
  interval: 2500,
});
// Монтируется последним: его первый refresh выбирает инструмент, а selectInstrument
// обращается к book/stats/panel/trades — они должны уже существовать.
// Сайт (тенант) — из своего URL. Для витрины это фильтр по allow-list; настоящая
// граница стоит на сервере, на пути открытия сделки: `?site=` подставит кто угодно.
const SITE = new URLSearchParams(location.search).get('site') || '';
const watchlist = Watchlist.mount(document.querySelector('.watch'), {
  fetch: async () => {
    const list = await api.instruments(SITE);
    // null здесь — «список не отдали» (например, конфиг сайта не доехал). Пустой
    // список читался бы как «сайту нечем торговать» — другое утверждение (BUG-011).
    markMarketData('instruments', list !== null);
    return list;
  },
  instrument: () => selected,
  onData: (list) => { list.forEach((it) => (META[it.id] = it)); updateDealPanel(); },
  onSelect: (id) => selectInstrument(id),
  interval: 1000,
});
async function selectInstrument(id) { selected = id; watchlist.setActive(id); /** @type {HTMLInputElement} */ (document.getElementById('price')).value = ''; /** @type {HTMLInputElement} */ (document.getElementById('sl')).value = ''; /** @type {HTMLInputElement} */ (document.getElementById('tp')).value = ''; await loadCandles(id, tf); updateDealPanel(); pollAccount(); panel.refreshDeals(); book.reset(); book.refresh(); stats.refresh(); trades.clearTape(); trades.refreshMine(); }
document.getElementById('tfs').addEventListener('click', (e) => { const btn = /** @type {Element} */ (e.target).closest('button'); if (!btn) return; tf = +btn.dataset.tf; document.querySelectorAll('#tfs button').forEach((b) => b.classList.toggle('active', b === btn)); if (selected != null) loadCandles(selected, tf); });

// ============================ Toolbar (dropdowns) ==========================
function closeDD() { document.querySelectorAll('.dd').forEach((d) => d.classList.add('hidden')); }
function toggleDD(id) { const m = document.getElementById(id); const wasHidden = m.classList.contains('hidden'); closeDD(); if (wasHidden) m.classList.remove('hidden'); }
document.addEventListener('click', (e) => { if (!/** @type {Element} */ (e.target).closest('.tb-dd')) closeDD(); });

document.getElementById('ctBtn').addEventListener('click', () => toggleDD('ctMenu'));
document.getElementById('ctMenu').addEventListener('click', (e) => { const it = /** @type {Element} */ (e.target).closest('[data-ct]'); if (!it) return; chartType = /** @type {HTMLElement} */ (it).dataset.ct; document.querySelectorAll('#ctMenu .dd-item').forEach((x) => x.classList.toggle('active', x === it)); const icName = (/** @type {HTMLElement} */ (it.querySelector('[data-icon]')))?.dataset.icon || 'candles'; const btnIc = /** @type {HTMLElement} */ (document.querySelector('#ctBtn .ic')); if (btnIc) { btnIc.dataset.icon = icName; btnIc.innerHTML = ICONS[icName]; } document.getElementById('ctLabel').textContent = it.textContent.trim(); closeDD(); makeMainSeries(); mainSeries.setData(mainData()); applyIndicators(); redrawPriceLines(); });

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

// Reset (full) / Fullscreen / Settings
function resetChart() {
  chartType = 'candles';
  Object.keys(overlays).forEach((k) => (overlays[k] = 0));
  oscillator = '';
  document.querySelectorAll('#indMenu [data-ov]').forEach((x) => x.classList.remove('on'));
  document.querySelectorAll('#indMenu .osc').forEach((x) => x.classList.toggle('on', /** @type {HTMLElement} */ (x).dataset.osc === ''));
  document.querySelectorAll('#ctMenu .dd-item').forEach((x) => x.classList.toggle('active', /** @type {HTMLElement} */ (x).dataset.ct === 'candles'));
  const btnIc = document.querySelector('#ctBtn .ic'); if (btnIc) { /** @type {HTMLElement} */ (btnIc).dataset.icon = 'candles'; btnIc.innerHTML = ICONS.candles; }
  document.getElementById('ctLabel').textContent = 'Candles';
  makeMainSeries(); mainSeries.setData(mainData()); applyIndicators(); redrawPriceLines();
  const n = rawCandles.length; if (n) chart.timeScale().setVisibleLogicalRange({ from: Math.max(0, n - 90), to: n + 3 });
}
document.getElementById('resetBtn').addEventListener('click', resetChart);
document.getElementById('fullBtn').addEventListener('click', () => { const cw = document.querySelector('.chartwrap'); if (document.fullscreenElement) document.exitFullscreen(); else cw.requestFullscreen?.(); });
document.getElementById('setBtn').addEventListener('click', () => toggleDD('setMenu'));
function bindSetting(id, key, kind) {
  const el = document.getElementById(id); if (!el) return;
  if (kind === 'bool') /** @type {HTMLInputElement} */ (el).checked = settings[key]; else /** @type {HTMLInputElement} */ (el).value = settings[key];
  el.addEventListener('change', () => { settings[key] = kind === 'bool' ? /** @type {HTMLInputElement} */ (el).checked : (kind === 'num' ? Number(/** @type {HTMLInputElement} */ (el).value) : /** @type {HTMLInputElement} */ (el).value); saveSettings(); applySettings(); });
}
bindSetting('sHollow', 'hollowColor', 'str'); bindSetting('sShadows', 'shadows', 'bool'); bindSetting('sShadowCustom', 'shadowCustom', 'bool'); bindSetting('sShadowColor', 'shadowColor', 'str');
bindSetting('sPlw', 'priceLineWidth', 'num'); bindSetting('sAddLine', 'addLine', 'bool'); bindSetting('sAlw', 'addLineWidth', 'num'); bindSetting('sMinChange', 'minChange', 'str');

// Видимость колонок списка: состояние и отрисовка — внутри виджета, страница лишь
// связывает свои чекбоксы с его публичным API.
function bindCol(id, key) { const el = document.getElementById(id); if (!el) return; /** @type {HTMLInputElement} */ (el).checked = !!watchlist.columns()[key]; el.addEventListener('change', () => watchlist.setColumns({ [key]: /** @type {HTMLInputElement} */ (el).checked ? 1 : 0 })); }
bindCol('cChange', 'change'); bindCol('cSell', 'sell'); bindCol('cBuy', 'buy'); bindCol('cSpread', 'spread'); bindCol('cHigh', 'high');

// Panel/toolbar resizing
// leftW=440: при 280px шесть колонок списка не помещались и цены резались многоточием (BUG-009).
// Замер по всем 30 строкам: 360px → 30 обрезанных ячеек, 400 → 6, 440 → 3, полный ноль только
// при 520px. 520 отдаёт списку 40% экрана и душит график, поэтому берём 440 как баланс.
// Сохранённый пользователем размер (localStorage) имеет приоритет над этим дефолтом.
const layout = Object.assign({ leftW: 300, rightW: 340, bottomH: 190, toolbarH: 42, dealH: 150, tradesH: 240 }, JSON.parse(localStorage.getItem('layout') || '{}'));
function applyLayout() {
  /** @type {HTMLElement} */ (document.querySelector('.grid')).style.gridTemplateColumns = `${layout.leftW}px 6px 1fr 6px ${layout.rightW}px`;
  /** @type {HTMLElement} */ (document.querySelector('.bottom')).style.height = layout.bottomH + 'px';
  /** @type {HTMLElement} */ (document.querySelector('.deal')).style.height = layout.dealH + 'px';
  document.getElementById('trades').style.height = layout.tradesH + 'px';
  // minHeight, а не height: тулбар должен иметь право вырасти, когда кнопки не влезли
  // в одну строку. Жёсткая высота выталкивала их поверх панели метрик (BUG-010).
  /** @type {HTMLElement} */ (document.querySelector('.toolbar')).style.minHeight = layout.toolbarH + 'px';
}
function initResizers() {
  document.querySelectorAll('[data-rsz]').forEach((h) => h.addEventListener('mousedown', (e) => {
    e.preventDefault(); h.classList.add('drag');
    const type = /** @type {HTMLElement} */ (h).dataset.rsz, sx = /** @type {MouseEvent} */ (e).clientX, sy = /** @type {MouseEvent} */ (e).clientY, sl = layout.leftW, sr = layout.rightW, sb = layout.bottomH, stH = layout.toolbarH, sd = layout.dealH, str = layout.tradesH;
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
document.addEventListener('mouseover', (e) => { const t = /** @type {Element} */ (e.target).closest('[data-tip]'); if (!t) return; tip.textContent = /** @type {HTMLElement} */ (t).dataset.tip; tip.classList.remove('hidden'); const r = t.getBoundingClientRect(); tip.style.left = r.left + r.width / 2 + 'px'; tip.style.top = r.bottom + 7 + 'px'; });
document.addEventListener('mouseout', (e) => { if (/** @type {Element} */ (e.target).closest('[data-tip]')) tip.classList.add('hidden'); });

// ============================ WebSocket ====================================
// Индикатор живости горит по ПРИХОДУ ДАННЫХ, а не по открытому сокету. Открытый
// сокет, по которому ничего не идёт, — это молчащий фид: если Binance отвалится,
// соединение браузер→gateway останется целым, и зелёная точка врала бы (BUG-011).
const FEED_STALE_MS = 20000;
let lastFeedMsgAt = 0;
let wsOpen = false;
/** Было ли живо на прошлой перерисовке — чтобы рвать ряд на переходе, а не постоянно. */
let wasFeedLive = false;
/**
 * Сколько сообщений фида не разобралось **за сессию**. Ноль — не «всё хорошо»,
 * а «пока ни одного»: различать их помогает возраст последнего сообщения.
 *
 * Счётчик НЕ взят из общего помощника сознательно, и это не недосмотр.
 * Помощник считает отказы **подряд** и обнуляет их при восстановлении — это
 * верно для опроса, где важно «источник лежит сейчас». Здесь же величина
 * другая: сколько сообщений **потеряно** за сессию. Обнулять её на первом
 * здоровом сообщении значило бы стирать след ровно тогда, когда он нужен.
 *
 * Политику ВЫВОДА при этом берём общую (TRM-17), с явно заданной частотой:
 * поток сообщений фида на порядок чаще опроса, и «на смене состояния» здесь
 * означало бы одну строку за всю сессию.
 */
let feedDropped = 0;
const feedDiag = window.Diag.reporter({ subject: 'feed', every: 50 });

/**
 * Разрыв ряда цены (TRM-19, Р-073): свеча закрывается, живые сделки до
 * пересева с сервера не принимаются.
 *
 * Непрерывность ломает не только свечу: направление в стакане считается от
 * предыдущей середины, и через дыру это сравнение с ценой из другого времени.
 * Ленту сделок НЕ чистим — она не ряд, а список произошедшего, и стирать
 * наблюдённое из-за обрыва связи нельзя.
 *
 * @param {string} reason одна из `Series.BREAK_REASONS`
 */
function breakPriceSeries(reason) {
  if (priceSeries.isBroken()) return;
  priceSeries.markBreak(reason);
  book.reset();
}

function renderFeedStatus() {
  const st = document.getElementById('status');
  const age = lastFeedMsgAt ? Date.now() - lastFeedMsgAt : Infinity;
  const live = wsOpen && age < FEED_STALE_MS;
  // Молчание дольше порога — такой же разрыв, как обрыв сокета: сокет цел, а
  // данных нет, и следующая сделка пришла бы в свечу из прошлого.
  //
  // Рвём НА ПЕРЕХОДЕ «было живо → стало молчать», а не всё время, пока молчит.
  // Разрыв снимается пересевом, и без перехода мы рвали бы ряд заново каждые
  // две секунды простоя, а каждое возвращение тянет полную перезагрузку свечей:
  // получился бы опрос тем чаще, чем хуже связь. Условие `lastFeedMsgAt`
  // отсекает старт страницы: там ряд ещё не начинался.
  if (lastFeedMsgAt && wasFeedLive && !live) breakPriceSeries('feed-stale');
  wasFeedLive = live;
  // Транспорт и данные — разные вопросы, и подсказка их различает: «сокет молчит»
  // и «шлюза нет» лечатся по-разному, хотя оба гасят точку.
  const why = !wsOpen
    ? 'no connection to terminal gateway'
    : live
      ? 'market data live'
      : `socket open, no market data for ${lastFeedMsgAt ? Math.round(age / 1000) + 's' : 'the whole session'}`;
  if (st) {
    st.className = 'status ' + (live ? 'status--on' : 'status--off');
    // Неразобранные сообщения приписываем к подсказке, а не прячем: зелёная
    // точка при сыплющемся разборе — ровно то «правильное на вид состояние»,
    // которое мы ловим.
    st.title = feedDropped ? `${why}; не разобрано сообщений: ${feedDropped}` : why;
  }
  // Слово рядом с названием биржи — тоже утверждение о состоянии, а не украшение.
  const label = document.getElementById('feedLabel');
  if (label) {
    label.textContent = live ? 'LIVE' : 'NO DATA';
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
      // Мягкий разбор означает «не падает», а не «исчезает без следа»
      // (TRM-16). Сообщение, которое не разобралось, — это либо наш сломанный
      // шлюз, либо чужой формат; без счётчика и строки в логе они неотличимы
      // друг от друга и от «биржа молчит». Счёт виден в подсказке индикатора
      // фида, так что след остаётся и в интерфейсе, а не только в консоли.
      feedDropped += 1;
      feedDiag.fail('malformed', String(ev.data).slice(0, 200));
      renderFeedStatus();
      return;
    }
    lastFeedMsgAt = Date.now();
    renderFeedStatus();
    for (const x of e) if (x.type === 'trade') { onLiveTrade(x.instrument, x.price, x.qty); trades.pushMarket(x); }
  };
}
// Точка обязана гаснуть сама: без этого «живо» осталось бы висеть с последнего тика.
setInterval(renderFeedStatus, 2000);

// ============================ Order form ===================================
document.getElementById('tabDeal').addEventListener('click', () => setMode('market'));
document.getElementById('tabLimit').addEventListener('click', () => setMode('limit'));
function setMode(m) { orderMode = m; document.getElementById('tabDeal').classList.toggle('active', m === 'market'); document.getElementById('tabLimit').classList.toggle('active', m === 'limit'); document.getElementById('priceRow').classList.toggle('hidden', m !== 'limit'); updateDealPanel(); }
document.getElementById('lotMinus').addEventListener('click', () => stepLot(-1));
document.getElementById('lotPlus').addEventListener('click', () => stepLot(1));
function stepLot(d) { const i = document.getElementById('lot'); if (sizeUnit === 'usd') /** @type {HTMLInputElement} */ (i).value = Math.max(1, (parseFloat(/** @type {HTMLInputElement} */ (i).value) || 0) + d * 10).toFixed(0); else /** @type {HTMLInputElement} */ (i).value = Math.max(0.001, (parseFloat(/** @type {HTMLInputElement} */ (i).value) || 0) + d * 0.01).toFixed(3); updateSizeEq(); }
function readSlTp(id) { const sl = parseFloat(/** @type {HTMLInputElement} */ (document.getElementById('sl')).value), tp = parseFloat(/** @type {HTMLInputElement} */ (document.getElementById('tp')).value); return { sl: sl > 0 ? Math.round(sl * pscale(id)) : null, tp: tp > 0 ? Math.round(tp * pscale(id)) : null }; }
// Размер сделки в базовых единицах (учитывает выбор LOT/USD).
function sizeBaseQty(id) { const v = parseFloat(/** @type {HTMLInputElement} */ (document.getElementById('lot')).value) || 0; if (v <= 0) return 0; if (sizeUnit === 'usd') { const p = refPrice(META[id]) / pscale(id); return p > 0 ? v / p : 0; } return v; }
/**
 * Эквивалент размера: сколько это в другой валюте (TRM-24).
 *
 * Цена может быть НЕИЗВЕСТНА — тикера по инструменту ещё нет. Раньше в этом
 * случае печаталось «≈ 0»: ноль читается как «ноль объёма», а не «цены не
 * знаем», и стоит это в панели ввода ордера, где человек принимает решение о
 * деньгах. Та же семья, что `change` с нулём вместо прочерка.
 */
function updateSizeEq() {
  const it = META[selected];
  const el = document.getElementById('sizeEq');
  if (!it) { el.textContent = ''; el.title = ''; return; }

  const p = priceOrNull(it);
  const v = parseFloat(/** @type {HTMLInputElement} */ (document.getElementById('lot')).value) || 0;
  const base = it.symbol.split('-')[0];

  if (p === null) {
    el.textContent = sizeUnit === 'usd' ? `≈ — ${base}` : '≈ $—';
    el.title = 'Цена инструмента неизвестна — пересчитать нечем';
    return;
  }

  el.title = '';
  el.textContent = sizeUnit === 'usd'
    ? `≈ ${(v / p).toFixed(qdec(it.id))} ${base}`
    : `≈ $${(v * p).toLocaleString('en-US', { maximumFractionDigits: 2 })}`;
}

/** Цена инструмента в единицах отображения или `null`, если её нет. */
function priceOrNull(it) {
  const raw = refPrice(it);
  if (raw == null) return null;
  const p = raw / pscale(it.id);
  return Number.isFinite(p) && p > 0 ? p : null;
}
/**
 * Смена единицы ввода. При неизвестной цене пересчитать нечем — и тогда мы
 * **не трогаем введённое число** (TRM-24).
 *
 * Раньше пересчёт шёл через `p = 0`, и поле молча становилось нулём: человек
 * набрал объём, переключил единицу и потерял его без единого сообщения. Это
 * хуже неверной подписи — это правка того, что ввёл человек.
 */
function setUnit(u) {
  if (sizeUnit === u) return;
  const it = META[selected];
  const inp = /** @type {HTMLInputElement} */ (document.getElementById('lot'));
  const v = parseFloat(inp.value) || 0;
  const p = it ? priceOrNull(it) : null;

  if (p !== null) {
    inp.value = u === 'usd' ? (v * p).toFixed(0) : (v / p).toFixed(3);
  }
  inp.step = u === 'usd' ? '1' : '0.001';

  sizeUnit = u;
  document.getElementById('unitLot').classList.toggle('active', u === 'lot');
  document.getElementById('unitUsd').classList.toggle('active', u === 'usd');
  updateSizeEq();
}
document.getElementById('unitLot').addEventListener('click', () => setUnit('lot'));
document.getElementById('unitUsd').addEventListener('click', () => setUnit('usd'));
document.getElementById('lot').addEventListener('input', updateSizeEq);
// Отказ границы объясняется словами, а не кодом: «инструмент не разрешён сайту» и
// «разрешения не доехали» — разные вещи. Первое окончательно, второе чинится
// администратором, и человек должен понимать, ждать ему или нет (ADR-023 Т3).
function rejectionText(status, code) {
  if (code === 'instrument_not_allowed') return 'This instrument is not available on this site';
  if (code === 'tenant_instruments_unavailable') return 'Instrument permissions unavailable — trading is closed';
  if (status === 401) return 'No cabinet session — trading unavailable';
  if (code === 'insufficient_margin') return 'Not enough free margin';
  return `Rejected ${status}: ${code || ''}`;
}

async function submit(side) {
  const id = selected; if (id == null) return; const msg = document.getElementById('dealMsg');
  const baseQty = sizeBaseQty(id);
  if (!(baseQty > 0)) { msg.textContent = 'Enter size'; msg.style.color = 'var(--down)'; return; }
  const { sl, tp } = readSlTp(id); const qty = Math.round(baseQty * qscale(id)); let body;
  const isLimit = orderMode === 'limit';
  if (isLimit) { const price = parseFloat(/** @type {HTMLInputElement} */ (document.getElementById('price')).value); if (!(price > 0)) { msg.textContent = 'Enter price'; msg.style.color = 'var(--down)'; return; } body = { instrument: id, side, qty, price: Math.round(price * pscale(id)), sl, tp }; }
  else body = { instrument: id, side, qty, sl, tp };
  const { ok, status, data } = isLimit ? await api.placePending(body) : await api.openDeal(body);
  if (ok) { msg.textContent = isLimit ? `Limit order #${data.id}` : `Opened #${data.id}: ${side.toUpperCase()} @ ${fmtP(id, data.entry)}`; msg.style.color = 'var(--up)'; panel.show(isLimit ? 'pending' : 'deals'); pollAccount(); panel.refresh(); }
  else { msg.textContent = rejectionText(status, data?.error); msg.style.color = 'var(--down)'; }
}
document.getElementById('btnBuy').addEventListener('click', () => submit('buy'));
document.getElementById('btnSell').addEventListener('click', () => submit('sell'));

// ============================ Account / positions / history ================
// Метрики счёта. Отказ показывается отказом, а не нулём: ноль — правдоподобный
// баланс, по нему принимают решение, а по прочерку с причиной идут разбираться
// (BUG-011). 'unreachable' оставляет прежние цифры: связь могла моргнуть, и
// затирать верные значения на каждый блип — хуже, чем показать их с оговоркой.
const ACCT_MSG = {
  'no-session': 'NO CABINET SESSION — TRADING UNAVAILABLE',
  unreachable: 'ACCOUNT UNREACHABLE — FIGURES MAY BE STALE',
  unknown: 'NO ACCOUNT DATA YET',
};
function showAcctWarn(status) {
  const el = document.getElementById('acctWarn');
  if (!el) return;
  const msg = status === 'ok' ? null : ACCT_MSG[status] || ACCT_MSG.unknown;
  el.hidden = !msg;
  el.textContent = msg || '';
}
// Модель ведения позиций счёта. В хеджировании встречная сделка открывает вторую
// позицию, в неттинге — сворачивает существующую: разница видна на деньгах, поэтому
// подпись стоит в шапке, а не в настройках (ADR-025).
function showPositionModel(model) {
  const el = document.getElementById('modelBadge');
  if (!el) return;
  if (!model) { el.hidden = true; return; }
  const netting = model === 'netting';
  el.hidden = false;
  el.textContent = netting ? 'NETTING' : 'HEDGING';
  el.title = netting
    ? 'Opposite trade reduces or closes your position'
    : 'Opposite trade opens a second, separate position — it does not close this one';
}

function blankMetrics() {
  ['mBalance', 'mEquity', 'mMargin', 'mFree', 'mPnl'].forEach((id) => {
    const el = document.getElementById(id); if (el) { el.textContent = '—'; el.style.color = ''; }
  });
}
async function pollAccount() {
  try {
    const a = await api.account();
    const st = api.tradingStatus();
    showAcctWarn(st);
    if (!a) {
      // Сессии нет — цифр нет и быть не может: показываем прочерки, а не последнее
      // известное значение (оно было бы от чужого/прошлого счёта).
      if (st === 'no-session' || st === 'unknown') blankMetrics();
      return;
    }
    showPositionModel(a.model);
    document.getElementById('mBalance').textContent = usd(a.balance);
    document.getElementById('mEquity').textContent = usd(a.equity);
    document.getElementById('mMargin').textContent = usd(a.used_margin);
    document.getElementById('mFree').textContent = usd(a.free_margin);
    const p = document.getElementById('mPnl');
    p.textContent = usd(a.open_pnl);
    p.style.color = a.open_pnl > 0 ? 'var(--up)' : a.open_pnl < 0 ? 'var(--down)' : '';
  } catch (e) {
    // Было `catch (e) { }` — глушилось всё, включая ошибку формы ответа счёта
    // (TRM-16). `api.account()` сеть уже проглатывает сам и возвращает `null`,
    // поэтому сюда долетает не блип, а дефект: отсутствующий узел разметки или
    // поле не того типа. Такое не проходит само и обязано быть громким.
    console.error('[account] метрики счёта не отрисованы', e);
  }
}
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

// Перетемизация графика при смене темы/бренда (theming-pack, ADR-023): структурные
// цвета + пересбор серии (её цвета читаются из токенов при создании) + перекраска
// объёмов. Тема-каналы уже применены на :root к моменту события.
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
    volume.setData(rawCandles.map((c) => ({ time: c.time, value: c.volume, color: volColor(c) })));
  }
}
document.addEventListener('themechange', applyChartTheme);

// ============================ Start ========================================
// Терминал автономный: без логина и общей шапки — открывается и работает сразу.
renderIcons();
applyLayout();
initResizers();
// Тема и бренд сайта (theming-pack v1, ADR-023 фаза Т1). Без CMS остаёмся на палитре
// платформы; кнопка ◐ переключает тёмную/светлую.
document.getElementById('themeToggle')?.addEventListener('click', () => window.Theming.toggleTheme());
window.Theming.init();
// Вход по handoff-токену платформы (ADR-023 фаза Т2). Без родителя-кабинета токен не
// придёт — терминал работает на счёте по умолчанию (dev/standalone). При смене сессии
// сразу обновляем сводку счёта.
window.Sso.init().onChange(() => pollAccount());
showStorageMode();
setInterval(syncLast, 2000);
setInterval(pollAccount, 1000);
connectWS();
pollAccount();
