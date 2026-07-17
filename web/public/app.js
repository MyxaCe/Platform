'use strict';

// ============================ Состояние ====================================
let META = {};
let selected = null;
let tf = 3600;
let orderMode = 'market';   // market | limit
let chartType = 'candles';
let priceType = 'mid';      // mid | ask | bid
let showPips = false;
const overlays = { sma: false, ema: false, boll: false };
let oscillator = '';        // '' | rsi | atr | adx | macd
let rawCandles = [];        // {time, open, high, low, close, volume} в реальных ценах
let currentDeals = [];      // открытые позиции выбранного инструмента (для линий)
const rowEls = {};

const token = () => document.getElementById('userSel').value;
const pdec = (id) => (META[id]?.price_decimals ?? 2);
const qdec = (id) => (META[id]?.qty_decimals ?? 3);
const pscale = (id) => 10 ** pdec(id);
const qscale = (id) => 10 ** qdec(id);
const fmtP = (id, raw) => (raw / pscale(id)).toLocaleString('en-US', { minimumFractionDigits: pdec(id), maximumFractionDigits: pdec(id) });
const fmtQ = (id, raw) => (raw / qscale(id)).toFixed(qdec(id));
const usd = (cents) => '$' + (cents / 100).toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });

// ============================ Индикаторы ===================================
const toSeries = (arr, times) => arr.map((v, i) => (v == null ? null : { time: times[i], value: v })).filter(Boolean);
function sma(v, p) { const o = []; let s = 0; for (let i = 0; i < v.length; i++) { s += v[i]; if (i >= p) s -= v[i - p]; o.push(i >= p - 1 ? s / p : null); } return o; }
function ema(v, p) { const o = []; const k = 2 / (p + 1); let e; for (let i = 0; i < v.length; i++) { e = i === 0 ? v[0] : v[i] * k + e * (1 - k); o.push(i >= p - 1 ? e : null); } return o; }
function stddev(v, p) { const o = []; for (let i = 0; i < v.length; i++) { if (i < p - 1) { o.push(null); continue; } let m = 0; for (let j = i - p + 1; j <= i; j++) m += v[j]; m /= p; let s = 0; for (let j = i - p + 1; j <= i; j++) s += (v[j] - m) ** 2; o.push(Math.sqrt(s / p)); } return o; }
function rsi(v, p) { const o = new Array(v.length).fill(null); let ag = 0, al = 0; for (let i = 1; i < v.length; i++) { const c = v[i] - v[i - 1], g = Math.max(c, 0), l = Math.max(-c, 0); if (i <= p) { ag += g; al += l; if (i === p) { ag /= p; al /= p; o[i] = 100 - 100 / (1 + (al === 0 ? 100 : ag / al)); } } else { ag = (ag * (p - 1) + g) / p; al = (al * (p - 1) + l) / p; o[i] = 100 - 100 / (1 + (al === 0 ? 100 : ag / al)); } } return o; }
function trueRange(cs) { const t = []; for (let i = 0; i < cs.length; i++) t.push(i === 0 ? cs[0].high - cs[0].low : Math.max(cs[i].high - cs[i].low, Math.abs(cs[i].high - cs[i - 1].close), Math.abs(cs[i].low - cs[i - 1].close))); return t; }
function atr(cs, p) { const tr = trueRange(cs); const o = new Array(cs.length).fill(null); let a; for (let i = 0; i < cs.length; i++) { if (i < p) { if (i === p - 1) { let s = 0; for (let j = 0; j < p; j++) s += tr[j]; a = s / p; o[i] = a; } } else { a = (a * (p - 1) + tr[i]) / p; o[i] = a; } } return o; }
function adx(cs, p) {
  const n = cs.length, o = new Array(n).fill(null), tr = [], pdm = [], ndm = [];
  for (let i = 0; i < n; i++) { if (i === 0) { tr.push(0); pdm.push(0); ndm.push(0); continue; } const up = cs[i].high - cs[i - 1].high, dn = cs[i - 1].low - cs[i].low; pdm.push(up > dn && up > 0 ? up : 0); ndm.push(dn > up && dn > 0 ? dn : 0); tr.push(Math.max(cs[i].high - cs[i].low, Math.abs(cs[i].high - cs[i - 1].close), Math.abs(cs[i].low - cs[i - 1].close))); }
  let atrS = 0, pS = 0, nS = 0; const dx = new Array(n).fill(null);
  for (let i = 1; i < n; i++) { if (i <= p) { atrS += tr[i]; pS += pdm[i]; nS += ndm[i]; if (i === p) { const a = 100 * pS / (atrS || 1), b = 100 * nS / (atrS || 1); dx[i] = 100 * Math.abs(a - b) / (a + b || 1); } } else { atrS = atrS - atrS / p + tr[i]; pS = pS - pS / p + pdm[i]; nS = nS - nS / p + ndm[i]; const a = 100 * pS / (atrS || 1), b = 100 * nS / (atrS || 1); dx[i] = 100 * Math.abs(a - b) / (a + b || 1); } }
  let adxV, cnt = 0, sum = 0;
  for (let i = 1; i < n; i++) { if (dx[i] == null) continue; cnt++; if (cnt <= p) { sum += dx[i]; if (cnt === p) { adxV = sum / p; o[i] = adxV; } } else { adxV = (adxV * (p - 1) + dx[i]) / p; o[i] = adxV; } }
  return o;
}
function macd(v) { const e12 = ema(v, 12), e26 = ema(v, 26); return v.map((_, i) => (e12[i] != null && e26[i] != null ? e12[i] - e26[i] : null)); }

// ============================ График =======================================
const chartEl = document.getElementById('chart');
const chart = LightweightCharts.createChart(chartEl, {
  layout: { background: { color: '#0b0e14' }, textColor: '#6b7688' },
  grid: { vertLines: { color: 'rgba(31,39,53,.4)' }, horzLines: { color: 'rgba(31,39,53,.4)' } },
  rightPriceScale: { borderColor: '#1f2735' },
  timeScale: { borderColor: '#1f2735', timeVisible: true, secondsVisible: false },
  crosshair: { mode: LightweightCharts.CrosshairMode.Normal },
});
const volume = chart.addHistogramSeries({ priceFormat: { type: 'volume' }, priceScaleId: '', color: '#2b3648' });
volume.priceScale().applyOptions({ scaleMargins: { top: 0.85, bottom: 0 } });

let mainSeries = null;
const ovSeries = {};   // overlay line series по имени
let oscSeries = null;
let priceLines = [];
let lastBarTime = 0, formingRaw = null;

const volColor = (c) => (c.close >= c.open ? 'rgba(38,166,154,.4)' : 'rgba(239,83,80,.4)');

function makeMainSeries() {
  if (mainSeries) chart.removeSeries(mainSeries);
  if (chartType === 'candles' || chartType === 'hollow') {
    mainSeries = chart.addCandlestickSeries({
      upColor: chartType === 'hollow' ? 'rgba(0,0,0,0)' : '#26a69a', downColor: chartType === 'hollow' ? 'rgba(0,0,0,0)' : '#ef5350',
      borderUpColor: '#26a69a', borderDownColor: '#ef5350', wickUpColor: '#26a69a', wickDownColor: '#ef5350',
    });
  } else if (chartType === 'bars') {
    mainSeries = chart.addBarSeries({ upColor: '#26a69a', downColor: '#ef5350' });
  } else if (chartType === 'line') {
    mainSeries = chart.addLineSeries({ color: '#f0b90b', lineWidth: 2 });
  } else {
    mainSeries = chart.addAreaSeries({ lineColor: '#f0b90b', topColor: 'rgba(240,185,11,.25)', bottomColor: 'rgba(240,185,11,.02)' });
  }
  if (selected != null) mainSeries.applyOptions({ priceFormat: { type: 'price', precision: pdec(selected), minMove: 1 / pscale(selected) } });
}

function mainData() {
  const bars = (chartType === 'line' || chartType === 'area');
  return rawCandles.map((c) => (bars ? { time: c.time, value: c.close } : { time: c.time, open: c.open, high: c.high, low: c.low, close: c.close }));
}

function applyIndicators() {
  const times = rawCandles.map((c) => c.time);
  const close = rawCandles.map((c) => c.close);
  const want = { sma: overlays.sma, ema: overlays.ema, bollU: overlays.boll, bollL: overlays.boll };
  const compute = { sma: () => sma(close, 20), ema: () => ema(close, 20) };
  // Overlays SMA/EMA
  for (const key of ['sma', 'ema']) {
    if (want[key]) {
      if (!ovSeries[key]) ovSeries[key] = chart.addLineSeries({ color: key === 'sma' ? '#4fc3f7' : '#ba68c8', lineWidth: 1, priceLineVisible: false, lastValueVisible: false });
      ovSeries[key].setData(toSeries(compute[key](), times));
    } else if (ovSeries[key]) { chart.removeSeries(ovSeries[key]); delete ovSeries[key]; }
  }
  // Bollinger
  if (overlays.boll) {
    const mid = sma(close, 20), sd = stddev(close, 20);
    const up = mid.map((m, i) => (m != null && sd[i] != null ? m + 2 * sd[i] : null));
    const lo = mid.map((m, i) => (m != null && sd[i] != null ? m - 2 * sd[i] : null));
    for (const [k, data] of [['bollU', up], ['bollM', mid], ['bollL', lo]]) {
      if (!ovSeries[k]) ovSeries[k] = chart.addLineSeries({ color: 'rgba(120,144,156,.7)', lineWidth: 1, priceLineVisible: false, lastValueVisible: false });
      ovSeries[k].setData(toSeries(data, times));
    }
  } else { for (const k of ['bollU', 'bollM', 'bollL']) if (ovSeries[k]) { chart.removeSeries(ovSeries[k]); delete ovSeries[k]; } }
  // Осциллятор (нижняя суб-панель)
  if (oscSeries) { chart.removeSeries(oscSeries); oscSeries = null; }
  if (oscillator) {
    volume.applyOptions({ visible: false });
    let arr;
    if (oscillator === 'rsi') arr = rsi(close, 14);
    else if (oscillator === 'atr') arr = atr(rawCandles, 14);
    else if (oscillator === 'adx') arr = adx(rawCandles, 14);
    else arr = macd(close);
    oscSeries = chart.addLineSeries({ color: '#f0b90b', lineWidth: 1, priceScaleId: 'osc', priceLineVisible: false, lastValueVisible: false });
    oscSeries.priceScale().applyOptions({ scaleMargins: { top: 0.78, bottom: 0 } });
    oscSeries.setData(toSeries(arr, times));
  } else { volume.applyOptions({ visible: true }); }
}

function redrawPriceLines() {
  if (!mainSeries) return;
  priceLines.forEach((l) => mainSeries.removePriceLine(l));
  priceLines = [];
  const s = pscale(selected);
  for (const d of currentDeals) {
    priceLines.push(mainSeries.createPriceLine({ price: d.entry / s, color: '#8899aa', lineStyle: 2, lineWidth: 1, title: `${d.side === 'buy' ? 'L' : 'S'} entry` }));
    if (d.sl != null) priceLines.push(mainSeries.createPriceLine({ price: d.sl / s, color: '#ef5350', lineStyle: 0, lineWidth: 1, title: 'SL' }));
    if (d.tp != null) priceLines.push(mainSeries.createPriceLine({ price: d.tp / s, color: '#26a69a', lineStyle: 0, lineWidth: 1, title: 'TP' }));
  }
}

new ResizeObserver(() => chart.applyOptions({ width: chartEl.clientWidth, height: chartEl.clientHeight })).observe(chartEl);
chart.subscribeCrosshairMove((p) => {
  const d = p.seriesData?.get(mainSeries); const id = selected; if (!d || id == null) return;
  const g = (x) => (x != null ? fmtP(id, x * pscale(id)) : '—');
  document.getElementById('ohlc').innerHTML = d.close != null
    ? `O <b>${g(d.open)}</b> H <b>${g(d.high)}</b> L <b>${g(d.low)}</b> C <b>${g(d.close)}</b>`
    : `<b>${g(d.value)}</b>`;
});

async function loadCandles(id, timeframe) {
  const r = await fetch(`/candles/${id}?tf=${timeframe}&limit=300`);
  const raw = await r.json().catch(() => []);
  const s = pscale(id), v = qscale(id);
  rawCandles = raw.map((c) => ({ time: c.time, open: c.open / s, high: c.high / s, low: c.low / s, close: c.close / s, volume: c.volume / v }));
  makeMainSeries();
  mainSeries.setData(mainData());
  volume.setData(rawCandles.map((c) => ({ time: c.time, value: c.volume, color: volColor(c) })));
  applyIndicators();
  redrawPriceLines();
  computeSignals();
  if (rawCandles.length) {
    const last = raw[raw.length - 1];
    lastBarTime = last.time; formingRaw = { ...last };
    const n = rawCandles.length;
    chart.timeScale().setVisibleLogicalRange({ from: Math.max(0, n - 90), to: n + 3 });
  } else { lastBarTime = 0; formingRaw = null; }
}

function drawForming() {
  if (!formingRaw || !mainSeries) return;
  const s = pscale(selected);
  const bars = (chartType === 'line' || chartType === 'area');
  mainSeries.update(bars ? { time: formingRaw.time, value: formingRaw.close / s } : { time: formingRaw.time, open: formingRaw.open / s, high: formingRaw.high / s, low: formingRaw.low / s, close: formingRaw.close / s });
}

function onLiveTrade(id, price, qty) {
  if (id !== selected) return;
  const now = Math.floor(Date.now() / 1000);
  const b = now - (now % tf);
  if (!formingRaw || b > formingRaw.time) formingRaw = { time: b, open: price, high: price, low: price, close: price, volume: qty };
  else { formingRaw.high = Math.max(formingRaw.high, price); formingRaw.low = Math.min(formingRaw.low, price); formingRaw.close = price; }
  if (formingRaw.time >= lastBarTime) { lastBarTime = formingRaw.time; drawForming(); }
}

async function syncLast() {
  const id = selected; if (id == null) return;
  const r = await fetch(`/candles/${id}?tf=${tf}&limit=2`);
  const raw = await r.json().catch(() => []); if (!raw.length) return;
  const last = raw[raw.length - 1];
  if (last.time >= lastBarTime) { lastBarTime = last.time; formingRaw = { ...last }; drawForming(); }
}

// ============================ Инструменты ==================================
async function refreshInstruments() {
  const r = await fetch('/instruments'); const list = await r.json().catch(() => []);
  const cont = document.getElementById('instruments');
  for (const it of list) {
    META[it.id] = it;
    let el = rowEls[it.id];
    if (!el) { el = document.createElement('div'); el.className = 'irow'; el.addEventListener('click', () => selectInstrument(it.id)); cont.appendChild(el); rowEls[it.id] = el; if (selected === null) selectInstrument(it.id); }
    const up = it.change >= 0; const [b, q] = it.symbol.split('-');
    el.innerHTML = `<div class="sym">${b}<small>/${q || 'USDT'}</small></div>` +
      `<div class="chg ta-r ${up ? 'up' : 'down'}">${up ? '+' : ''}${it.change.toFixed(2)}%</div>` +
      `<div class="sell ta-r">${it.bid != null ? fmtP(it.id, it.bid) : '—'}</div>` +
      `<div class="buy ta-r">${it.ask != null ? fmtP(it.id, it.ask) : '—'}</div>` +
      `<div class="ta-r muted">${it.bid != null && it.ask != null ? (it.ask - it.bid) : '—'}</div>`;
    el.classList.toggle('active', it.id === selected);
  }
  updateDealPanel();
}

function refPrice(it) {
  if (!it || it.bid == null || it.ask == null) return it?.last;
  if (priceType === 'ask') return it.ask;
  if (priceType === 'bid') return it.bid;
  return Math.round((it.bid + it.ask) / 2);
}

function updateDealPanel() {
  const it = META[selected]; if (!it) return;
  document.getElementById('dealSym').textContent = it.symbol;
  const ref = refPrice(it);
  document.getElementById('dealSub').textContent = `${priceType.toUpperCase()} ${ref != null ? fmtP(it.id, ref) : '—'}`;
  const chg = document.getElementById('dealChange'); const up = it.change >= 0;
  chg.textContent = `${up ? '+' : ''}${it.change.toFixed(2)}%`; chg.className = `deal__change ${up ? 'up' : 'down'}`;
  document.getElementById('askPx').textContent = it.ask != null ? fmtP(it.id, it.ask) : '—';
  document.getElementById('bidPx').textContent = it.bid != null ? fmtP(it.id, it.bid) : '—';
  document.getElementById('watermark').textContent = it.symbol;
  const pips = document.getElementById('pipsInfo');
  if (showPips && it.bid != null && it.ask != null) { pips.classList.remove('hidden'); pips.textContent = `Спред: ${it.ask - it.bid} pips`; }
  else pips.classList.add('hidden');
  if (orderMode === 'limit' && !document.getElementById('price').value && ref != null) document.getElementById('price').value = (ref / pscale(it.id)).toFixed(pdec(it.id));
}

// ============================ Выбор / таймфрейм ============================
async function selectInstrument(id) {
  selected = id;
  Object.entries(rowEls).forEach(([k, el]) => el.classList.toggle('active', +k === id));
  document.getElementById('price').value = ''; document.getElementById('sl').value = ''; document.getElementById('tp').value = '';
  await loadCandles(id, tf); updateDealPanel(); pollAccount(); pollDeals();
}
document.getElementById('tfs').addEventListener('click', (e) => {
  const btn = e.target.closest('button'); if (!btn) return;
  tf = +btn.dataset.tf;
  document.querySelectorAll('#tfs button').forEach((b) => b.classList.toggle('active', b === btn));
  if (selected != null) loadCandles(selected, tf);
});

// ============================ Тулбар =======================================
document.getElementById('chartType').addEventListener('change', (e) => { chartType = e.target.value; makeMainSeries(); mainSeries.setData(mainData()); applyIndicators(); redrawPriceLines(); });
document.getElementById('indBtn').addEventListener('click', () => document.getElementById('indMenu').classList.toggle('hidden'));
document.querySelectorAll('#indMenu input[data-ov]').forEach((c) => c.addEventListener('change', (e) => { overlays[e.target.dataset.ov] = e.target.checked; applyIndicators(); }));
document.getElementById('oscSel').addEventListener('change', (e) => { oscillator = e.target.value; applyIndicators(); });
document.getElementById('priceType').addEventListener('change', (e) => { priceType = e.target.value; updateDealPanel(); });
document.getElementById('pipsChk').addEventListener('change', (e) => { showPips = e.target.checked; updateDealPanel(); pollDeals(); });

// ============================ Лента + WS ===================================
function addTape(id, rawPrice, rawQty, side) {
  const el = document.getElementById('tape'); const div = document.createElement('div'); div.className = 't';
  const tm = new Date().toLocaleTimeString('ru-RU', { hour12: false });
  div.innerHTML = `<span>${META[id]?.symbol || id}</span><span class="px ${side}">${fmtP(id, rawPrice)}</span><span>${fmtQ(id, rawQty)}</span><span class="tm">${tm}</span>`;
  el.prepend(div); while (el.childElementCount > 60) el.removeChild(el.lastChild);
}
function connectWS() {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${proto}://${location.host}/stream`); const st = document.getElementById('status');
  ws.onopen = () => { st.className = 'status status--on'; };
  ws.onclose = () => { st.className = 'status status--off'; setTimeout(connectWS, 1500); };
  ws.onmessage = (ev) => { let e; try { e = JSON.parse(ev.data); } catch { return; } for (const x of e) if (x.type === 'trade') { addTape(x.instrument, x.price, x.qty, x.taker_side); onLiveTrade(x.instrument, x.price, x.qty); } };
}

// ============================ Форма сделки =================================
document.getElementById('tabDeal').addEventListener('click', () => setMode('market'));
document.getElementById('tabLimit').addEventListener('click', () => setMode('limit'));
function setMode(m) {
  orderMode = m;
  document.getElementById('tabDeal').classList.toggle('active', m === 'market');
  document.getElementById('tabLimit').classList.toggle('active', m === 'limit');
  document.getElementById('priceRow').classList.toggle('hidden', m !== 'limit');
  updateDealPanel();
}
document.getElementById('lotMinus').addEventListener('click', () => stepLot(-1));
document.getElementById('lotPlus').addEventListener('click', () => stepLot(1));
function stepLot(d) { const i = document.getElementById('lot'); i.value = Math.max(0.001, (parseFloat(i.value) || 0) + d * 0.01).toFixed(3); }

function readSlTp(id) {
  const sl = parseFloat(document.getElementById('sl').value); const tp = parseFloat(document.getElementById('tp').value);
  return { sl: sl > 0 ? Math.round(sl * pscale(id)) : null, tp: tp > 0 ? Math.round(tp * pscale(id)) : null };
}

async function submit(side) {
  const id = selected; if (id == null) return;
  const lot = parseFloat(document.getElementById('lot').value) || 0;
  const msg = document.getElementById('dealMsg');
  if (lot <= 0) { msg.textContent = 'Укажи объём'; msg.style.color = 'var(--down)'; return; }
  const { sl, tp } = readSlTp(id);
  const qty = Math.round(lot * qscale(id));
  let url, body;
  if (orderMode === 'limit') {
    const price = parseFloat(document.getElementById('price').value);
    if (!(price > 0)) { msg.textContent = 'Укажи цену'; msg.style.color = 'var(--down)'; return; }
    url = '/pending'; body = { instrument: id, side, qty, price: Math.round(price * pscale(id)), sl, tp };
  } else { url = '/deals'; body = { instrument: id, side, qty, sl, tp }; }
  const r = await fetch(url, { method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token()}` }, body: JSON.stringify(body) });
  const data = await r.json().catch(() => null);
  if (r.ok) {
    msg.textContent = orderMode === 'limit' ? `Лимитный ордер #${data.id}` : `Открыта #${data.id}: ${side.toUpperCase()} @ ${fmtP(id, data.entry)}`;
    msg.style.color = 'var(--up)';
    switchBottom(orderMode === 'limit' ? 'pending' : 'deals');
    pollAccount(); pollDeals(); pollPending();
  } else { msg.textContent = `Отказ ${r.status}: ${data?.error || ''}`; msg.style.color = 'var(--down)'; }
}
document.getElementById('btnBuy').addEventListener('click', () => submit('buy'));
document.getElementById('btnSell').addEventListener('click', () => submit('sell'));
document.getElementById('userSel').addEventListener('change', () => { pollAccount(); pollDeals(); pollPending(); pollClosed(); });

// ============================ Счёт / позиции / история =====================
async function pollAccount() {
  try {
    const r = await fetch('/account', { headers: { Authorization: `Bearer ${token()}` } }); if (!r.ok) return;
    const a = await r.json();
    document.getElementById('mBalance').textContent = usd(a.balance);
    document.getElementById('mEquity').textContent = usd(a.equity);
    document.getElementById('mMargin').textContent = usd(a.used_margin);
    document.getElementById('mFree').textContent = usd(a.free_margin);
    const p = document.getElementById('mPnl'); p.textContent = usd(a.open_pnl);
    p.style.color = a.open_pnl > 0 ? 'var(--up)' : a.open_pnl < 0 ? 'var(--down)' : '';
  } catch (e) { /* игнор */ }
}

async function pollDeals() {
  try {
    const r = await fetch('/deals', { headers: { Authorization: `Bearer ${token()}` } }); if (!r.ok) return;
    const list = await r.json();
    currentDeals = list.filter((d) => d.instrument === selected);
    redrawPriceLines();
    document.getElementById('deals').innerHTML = list.map((d) => {
      const pos = d.pnl >= 0; const lot = (d.qty / 10 ** d.qty_decimals).toFixed(d.qty_decimals);
      const f = (raw) => (raw / 10 ** d.price_decimals).toFixed(d.price_decimals);
      const pips = showPips ? ` (${d.side === 'buy' ? d.mark - d.entry : d.entry - d.mark} p)` : '';
      return `<div class="deal-row"><span>${d.symbol}</span><span class="sd ${d.side}">${d.side.toUpperCase()}</span>` +
        `<span>${lot}</span><span>${f(d.entry)}</span><span>${f(d.mark)}</span>` +
        `<span class="pnl ${pos ? 'pos' : 'neg'}">${usd(d.pnl)}${pips}</span>` +
        `<button class="closebtn" data-id="${d.id}">Close</button></div>`;
    }).join('') || '<div class="deal-row muted"><span>нет открытых позиций</span></div>';
    document.querySelectorAll('#deals .closebtn').forEach((b) => b.addEventListener('click', () => closeDeal(+b.dataset.id)));
  } catch (e) { /* игнор */ }
}
async function closeDeal(id) { const r = await fetch(`/deals/${id}/close`, { method: 'POST', headers: { Authorization: `Bearer ${token()}` } }); if (r.ok) { pollAccount(); pollDeals(); pollClosed(); } }

async function pollPending() {
  try {
    const r = await fetch('/pending', { headers: { Authorization: `Bearer ${token()}` } }); if (!r.ok) return;
    const list = await r.json();
    document.getElementById('pending').innerHTML = list.map((d) => {
      const lot = (d.qty / 10 ** d.qty_decimals).toFixed(d.qty_decimals); const f = (raw) => raw == null ? '—' : (raw / 10 ** d.price_decimals).toFixed(d.price_decimals);
      return `<div class="deal-row"><span>${d.symbol}</span><span class="sd ${d.side}">${d.side.toUpperCase()}</span>` +
        `<span>${lot}</span><span>${f(d.price)}</span><span>${f(d.sl)}</span><span>${f(d.tp)}</span>` +
        `<button class="closebtn" data-id="${d.id}">Cancel</button></div>`;
    }).join('') || '<div class="deal-row muted"><span>нет лимитных ордеров</span></div>';
    document.querySelectorAll('#pending .closebtn').forEach((b) => b.addEventListener('click', () => cancelPending(+b.dataset.id)));
  } catch (e) { /* игнор */ }
}
async function cancelPending(id) { const r = await fetch(`/pending/${id}`, { method: 'DELETE', headers: { Authorization: `Bearer ${token()}` } }); if (r.ok) pollPending(); }

async function pollClosed() {
  try {
    const r = await fetch('/deals/closed', { headers: { Authorization: `Bearer ${token()}` } }); if (!r.ok) return;
    const list = await r.json();
    document.getElementById('closed').innerHTML = list.map((d) => {
      const pos = d.pnl >= 0; const lot = (d.qty / 10 ** d.qty_decimals).toFixed(d.qty_decimals); const f = (raw) => (raw / 10 ** d.price_decimals).toFixed(d.price_decimals);
      return `<div class="deal-row"><span>${d.symbol}</span><span class="sd ${d.side}">${d.side.toUpperCase()}</span>` +
        `<span>${lot}</span><span>${f(d.entry)}</span><span>${f(d.exit)}</span><span class="pnl ${pos ? 'pos' : 'neg'}">${usd(d.pnl)}</span><span></span></div>`;
    }).join('') || '<div class="deal-row muted"><span>история пуста</span></div>';
  } catch (e) { /* игнор */ }
}

// ============================ Сигналы ======================================
function computeSignals() {
  const cont = document.getElementById('signals'); const it = META[selected]; if (!it || rawCandles.length < 30) { cont.innerHTML = ''; return; }
  const close = rawCandles.map((c) => c.close);
  const sig = [];
  const r = rsi(close, 14); const rv = r[r.length - 1];
  if (rv != null && rv > 70) sig.push(['RSI перекуплен', 'SELL']);
  else if (rv != null && rv < 30) sig.push(['RSI перепродан', 'BUY']);
  const f = sma(close, 9), s = sma(close, 21);
  const i = close.length - 1;
  if (f[i] != null && s[i] != null && f[i - 1] != null && s[i - 1] != null) {
    if (f[i - 1] <= s[i - 1] && f[i] > s[i]) sig.push(['MA 9/21 крест вверх', 'BUY']);
    else if (f[i - 1] >= s[i - 1] && f[i] < s[i]) sig.push(['MA 9/21 крест вниз', 'SELL']);
  }
  const tfName = { 60: '1m', 300: '5m', 900: '15m', 1800: '30m', 3600: '1h', 14400: '4h', 86400: '1d', 604800: '1w' }[tf];
  cont.innerHTML = sig.map(([name, dir]) =>
    `<div class="deal-row"><span>${it.symbol}</span><span>${name}</span><span>${tfName}</span><span class="dir ${dir === 'BUY' ? 'buy' : 'sell'} ta-r">${dir}</span></div>`
  ).join('') || '<div class="deal-row muted"><span>нет сигналов на этом ТФ</span></div>';
}

// ============================ Вкладки внизу + поиск ========================
function switchBottom(which) {
  ['trades', 'deals', 'pending', 'closed', 'signals'].forEach((n) => {
    document.getElementById(`tab-${n}`).classList.toggle('active', n === which);
    document.getElementById(`pane-${n}`).classList.toggle('hidden', n !== which);
  });
}
document.querySelectorAll('.bottom__tabs span').forEach((t) => t.addEventListener('click', () => switchBottom(t.dataset.pane)));
document.getElementById('search').addEventListener('input', (e) => {
  const q = e.target.value.toLowerCase();
  Object.entries(rowEls).forEach(([id, el]) => { el.style.display = (META[id]?.symbol || '').toLowerCase().includes(q) ? '' : 'none'; });
});

// ============================ Старт ========================================
refreshInstruments();
setInterval(refreshInstruments, 1000);
setInterval(syncLast, 2000);
setInterval(() => { pollAccount(); pollDeals(); }, 1000);
setInterval(() => { pollPending(); pollClosed(); }, 1500);
connectWS();
pollAccount(); pollDeals(); pollPending(); pollClosed();
