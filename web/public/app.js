'use strict';

// ---- Состояние ------------------------------------------------------------
let META = {};            // id -> {symbol, price_decimals, qty_decimals, ...}
let selected = null;      // текущий инструмент id
let tf = 3600;            // текущий таймфрейм (сек)
let orderMode = 'market'; // 'market' | 'limit'
const rowEls = {};        // id -> DOM строка списка

const token = () => document.getElementById('userSel').value;
const pdec = (id) => (META[id]?.price_decimals ?? 2);
const qdec = (id) => (META[id]?.qty_decimals ?? 3);
const pscale = (id) => 10 ** pdec(id);
const qscale = (id) => 10 ** qdec(id);
const fmtP = (id, raw) => (raw / pscale(id)).toLocaleString('en-US', { minimumFractionDigits: pdec(id), maximumFractionDigits: pdec(id) });
const fmtQ = (id, raw) => (raw / qscale(id)).toFixed(qdec(id));

// ---- График ---------------------------------------------------------------
const chartEl = document.getElementById('chart');
const chart = LightweightCharts.createChart(chartEl, {
  layout: { background: { color: '#0b0e14' }, textColor: '#6b7688' },
  grid: { vertLines: { color: 'rgba(31,39,53,.4)' }, horzLines: { color: 'rgba(31,39,53,.4)' } },
  rightPriceScale: { borderColor: '#1f2735' },
  timeScale: { borderColor: '#1f2735', timeVisible: true, secondsVisible: tf < 60 },
  crosshair: { mode: LightweightCharts.CrosshairMode.Normal },
});
const candles = chart.addCandlestickSeries({
  upColor: '#26a69a', downColor: '#ef5350', borderUpColor: '#26a69a',
  borderDownColor: '#ef5350', wickUpColor: '#26a69a', wickDownColor: '#ef5350',
});
const volume = chart.addHistogramSeries({ priceFormat: { type: 'volume' }, priceScaleId: '', color: '#2b3648' });
volume.priceScale().applyOptions({ scaleMargins: { top: 0.85, bottom: 0 } });
new ResizeObserver(() => chart.applyOptions({ width: chartEl.clientWidth, height: chartEl.clientHeight })).observe(chartEl);

let lastBar = null;
chart.subscribeCrosshairMove((p) => {
  const d = p.seriesData?.get(candles);
  const id = selected;
  if (d) document.getElementById('ohlc').innerHTML =
    `O <b>${fmtP(id, d.open * pscale(id))}</b> H <b>${fmtP(id, d.high * pscale(id))}</b> L <b>${fmtP(id, d.low * pscale(id))}</b> C <b>${fmtP(id, d.close * pscale(id))}</b>`;
});

async function loadCandles(id, timeframe) {
  const r = await fetch(`/candles/${id}?tf=${timeframe}&limit=300`);
  const raw = await r.json();
  const s = pscale(id), v = qscale(id);
  candles.setData(raw.map((c) => ({ time: c.time, open: c.open / s, high: c.high / s, low: c.low / s, close: c.close / s })));
  volume.setData(raw.map((c) => ({ time: c.time, value: c.volume / v, color: c.close >= c.open ? 'rgba(38,166,154,.4)' : 'rgba(239,83,80,.4)' })));
  lastBar = raw.length ? { ...raw[raw.length - 1] } : null;
  chart.timeScale().fitContent();
}

function onLiveTrade(id, rawPrice, rawQty) {
  if (id !== selected || !lastBar) return;
  const s = pscale(id);
  const now = Math.floor(Date.now() / 1000);
  const b = now - (now % tf);
  if (b > lastBar.time) lastBar = { time: b, open: rawPrice, high: rawPrice, low: rawPrice, close: rawPrice, volume: rawQty };
  else { lastBar.high = Math.max(lastBar.high, rawPrice); lastBar.low = Math.min(lastBar.low, rawPrice); lastBar.close = rawPrice; }
  candles.update({ time: lastBar.time, open: lastBar.open / s, high: lastBar.high / s, low: lastBar.low / s, close: lastBar.close / s });
}

// ---- Список инструментов --------------------------------------------------
async function refreshInstruments() {
  const r = await fetch('/instruments');
  const list = await r.json();
  const cont = document.getElementById('instruments');
  for (const it of list) {
    META[it.id] = it;
    let el = rowEls[it.id];
    if (!el) {
      el = document.createElement('div');
      el.className = 'irow';
      el.addEventListener('click', () => selectInstrument(it.id));
      cont.appendChild(el);
      rowEls[it.id] = el;
      if (selected === null) selectInstrument(it.id);
    }
    const up = it.change >= 0;
    el.innerHTML =
      `<div class="sym">${it.symbol.split('-')[0]}<small>/${it.symbol.split('-')[1] || 'USDT'}</small></div>` +
      `<div class="chg ta-r ${up ? 'up' : 'down'}">${up ? '+' : ''}${it.change.toFixed(2)}%</div>` +
      `<div class="sell ta-r">${it.bid != null ? fmtP(it.id, it.bid) : '—'}</div>` +
      `<div class="buy ta-r">${it.ask != null ? fmtP(it.id, it.ask) : '—'}</div>` +
      `<div class="ta-r muted">${it.bid != null && it.ask != null ? (it.ask - it.bid) : '—'}</div>`;
    el.classList.toggle('active', it.id === selected);
  }
  updateDealPanel();
  updateMetrics();
}

// ---- Панель сделки --------------------------------------------------------
function updateDealPanel() {
  const it = META[selected];
  if (!it) return;
  document.getElementById('dealSym').textContent = it.symbol;
  document.getElementById('dealSub').textContent = `база ${it.base} / котир. ${it.quote}`;
  const chg = document.getElementById('dealChange');
  const up = it.change >= 0;
  chg.textContent = `${up ? '+' : ''}${it.change.toFixed(2)}%`;
  chg.className = `deal__change ${up ? 'up' : 'down'}`;
  document.getElementById('askPx').textContent = it.ask != null ? fmtP(it.id, it.ask) : '—';
  document.getElementById('bidPx').textContent = it.bid != null ? fmtP(it.id, it.bid) : '—';
  document.getElementById('watermark').textContent = it.symbol;
  if (orderMode === 'limit' && !document.getElementById('price').value && it.last != null)
    document.getElementById('price').value = (it.last / pscale(it.id)).toFixed(pdec(it.id));
}

async function updateMetrics() {
  try {
    const it = META[selected]; if (!it) return;
    const r = await fetch(`/balance/${it.quote}`, { headers: { Authorization: `Bearer ${token()}` } });
    if (!r.ok) return;
    const b = await r.json();
    const s = pscale(it.id); // котируемый актив в тех же decimals (упрощённо)
    const av = b.available / s, eq = (b.available + b.held) / s;
    const f = (n) => '$' + n.toLocaleString('en-US', { maximumFractionDigits: 2 });
    document.getElementById('mBalance').textContent = f(av);
    document.getElementById('mEquity').textContent = f(eq);
    document.getElementById('mFree').textContent = f(av);
  } catch (e) { /* игнор */ }
}

// ---- Выбор инструмента / таймфрейма ---------------------------------------
async function selectInstrument(id) {
  selected = id;
  Object.entries(rowEls).forEach(([k, el]) => el.classList.toggle('active', +k === id));
  document.getElementById('price').value = '';
  await loadCandles(id, tf);
  updateDealPanel();
  updateMetrics();
}

document.getElementById('tfs').addEventListener('click', (e) => {
  const btn = e.target.closest('button'); if (!btn) return;
  tf = +btn.dataset.tf;
  document.querySelectorAll('#tfs button').forEach((b) => b.classList.toggle('active', b === btn));
  chart.applyOptions({ timeScale: { secondsVisible: tf < 60 } });
  if (selected != null) loadCandles(selected, tf);
});

// ---- Лента сделок ---------------------------------------------------------
function addTape(id, rawPrice, rawQty, side) {
  const el = document.getElementById('tape');
  const div = document.createElement('div');
  div.className = 't';
  const tm = new Date().toLocaleTimeString('ru-RU', { hour12: false });
  div.innerHTML = `<span>${META[id]?.symbol || id}</span><span class="px ${side}">${fmtP(id, rawPrice)}</span><span>${fmtQ(id, rawQty)}</span><span class="tm">${tm}</span>`;
  el.prepend(div);
  while (el.childElementCount > 60) el.removeChild(el.lastChild);
}

// ---- WebSocket ------------------------------------------------------------
function connectWS() {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${proto}://${location.host}/stream`);
  const st = document.getElementById('status');
  ws.onopen = () => { st.className = 'status status--on'; };
  ws.onclose = () => { st.className = 'status status--off'; setTimeout(connectWS, 1500); };
  ws.onmessage = (ev) => {
    let events; try { events = JSON.parse(ev.data); } catch { return; }
    for (const e of events) {
      if (e.type === 'trade') { addTape(e.instrument, e.price, e.qty, e.taker_side); onLiveTrade(e.instrument, e.price, e.qty); }
    }
  };
}

// ---- Форма сделки ---------------------------------------------------------
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
function stepLot(d) {
  const i = document.getElementById('lot');
  const v = Math.max(0.001, (parseFloat(i.value) || 0) + d * 0.01);
  i.value = v.toFixed(3);
}

async function submit(side) {
  const id = selected; if (id == null) return;
  const lot = parseFloat(document.getElementById('lot').value);
  const msg = document.getElementById('dealMsg');
  if (!lot || lot <= 0) { msg.textContent = 'Укажи лот'; msg.style.color = 'var(--down)'; return; }
  const body = { instrument: id, side, type: orderMode, qty: Math.round(lot * qscale(id)) };
  if (orderMode === 'limit') {
    const price = parseFloat(document.getElementById('price').value);
    if (!(price > 0)) { msg.textContent = 'Укажи цену'; msg.style.color = 'var(--down)'; return; }
    body.price = Math.round(price * pscale(id));
  }
  const r = await fetch('/orders', {
    method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token()}` },
    body: JSON.stringify(body),
  });
  const data = await r.json().catch(() => null);
  msg.textContent = r.ok ? `OK: заявка #${data.order_id}` : `Отказ ${r.status}: ${data?.error || ''}`;
  msg.style.color = r.ok ? 'var(--up)' : 'var(--down)';
  updateMetrics();
}
document.getElementById('btnBuy').addEventListener('click', () => submit('buy'));
document.getElementById('btnSell').addEventListener('click', () => submit('sell'));
document.getElementById('userSel').addEventListener('change', updateMetrics);

// ---- Поиск ----------------------------------------------------------------
document.getElementById('search').addEventListener('input', (e) => {
  const q = e.target.value.toLowerCase();
  Object.entries(rowEls).forEach(([id, el]) => {
    el.style.display = (META[id]?.symbol || '').toLowerCase().includes(q) ? '' : 'none';
  });
});

// ---- Старт ----------------------------------------------------------------
refreshInstruments();
setInterval(refreshInstruments, 1000);
setInterval(() => { if (selected != null) loadCandles(selected, tf); }, 4000);
connectWS();
