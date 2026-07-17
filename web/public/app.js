'use strict';

// ---- Состояние ------------------------------------------------------------
let META = {};            // id -> метаданные инструмента
let selected = null;      // текущий инструмент
let tf = 3600;            // таймфрейм (сек)
let orderMode = 'market';
const rowEls = {};

// Состояние графика
let lastBarTime = 0;      // время последней свечи на графике
let formingRaw = null;    // текущая формирующаяся свеча в RAW-ценах {time,open,high,low,close}

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
  timeScale: { borderColor: '#1f2735', timeVisible: true, secondsVisible: false },
  crosshair: { mode: LightweightCharts.CrosshairMode.Normal },
});
const candles = chart.addCandlestickSeries({
  upColor: '#26a69a', downColor: '#ef5350', borderUpColor: '#26a69a',
  borderDownColor: '#ef5350', wickUpColor: '#26a69a', wickDownColor: '#ef5350',
});
const volume = chart.addHistogramSeries({ priceFormat: { type: 'volume' }, priceScaleId: '', color: '#2b3648' });
volume.priceScale().applyOptions({ scaleMargins: { top: 0.85, bottom: 0 } });
new ResizeObserver(() => chart.applyOptions({ width: chartEl.clientWidth, height: chartEl.clientHeight })).observe(chartEl);

chart.subscribeCrosshairMove((p) => {
  const d = p.seriesData?.get(candles); const id = selected; if (!d || id == null) return;
  const s = pscale(id);
  document.getElementById('ohlc').innerHTML =
    `O <b>${fmtP(id, d.open * s)}</b> H <b>${fmtP(id, d.high * s)}</b> L <b>${fmtP(id, d.low * s)}</b> C <b>${fmtP(id, d.close * s)}</b>`;
});

const volColor = (c) => (c.close >= c.open ? 'rgba(38,166,154,.4)' : 'rgba(239,83,80,.4)');

/// Обновить последнюю свечу графика по RAW-свече (не трогает зум/вид).
function drawCandle(id, c) {
  const s = pscale(id), v = qscale(id);
  candles.update({ time: c.time, open: c.open / s, high: c.high / s, low: c.low / s, close: c.close / s });
  volume.update({ time: c.time, value: c.volume / v, color: volColor(c) });
}

/// Полная загрузка свечей — ТОЛЬКО при смене инструмента/таймфрейма. Здесь можно менять вид.
async function loadCandles(id, timeframe) {
  const r = await fetch(`/candles/${id}?tf=${timeframe}&limit=300`);
  const raw = await r.json().catch(() => []);
  const s = pscale(id), v = qscale(id);
  candles.applyOptions({ priceFormat: { type: 'price', precision: pdec(id), minMove: 1 / s } });
  candles.setData(raw.map((c) => ({ time: c.time, open: c.open / s, high: c.high / s, low: c.low / s, close: c.close / s })));
  volume.setData(raw.map((c) => ({ time: c.time, value: c.volume / v, color: volColor(c) })));
  if (raw.length) {
    const last = raw[raw.length - 1];
    lastBarTime = last.time;
    formingRaw = { ...last };
    const n = raw.length;
    chart.timeScale().setVisibleLogicalRange({ from: Math.max(0, n - 90), to: n + 3 }); // последние ~90 баров
  } else {
    lastBarTime = 0; formingRaw = null;
  }
}

/// Живое обновление формирующейся свечи из WS-сделки (без setData/зума).
function onLiveTrade(id, price, qty) {
  if (id !== selected) return;
  const now = Math.floor(Date.now() / 1000);
  const b = now - (now % tf);
  if (!formingRaw || b > formingRaw.time) {
    formingRaw = { time: b, open: price, high: price, low: price, close: price, volume: qty };
  } else {
    formingRaw.high = Math.max(formingRaw.high, price);
    formingRaw.low = Math.min(formingRaw.low, price);
    formingRaw.close = price;
    formingRaw.volume += qty;
  }
  if (formingRaw.time >= lastBarTime) { lastBarTime = formingRaw.time; drawCandle(id, formingRaw); }
}

/// Периодическая сверка последней свечи с сервером (авторитетно), без сброса зума.
async function syncLast() {
  const id = selected; if (id == null) return;
  const r = await fetch(`/candles/${id}?tf=${tf}&limit=2`);
  const raw = await r.json().catch(() => []);
  if (!raw.length) return;
  const last = raw[raw.length - 1];
  if (last.time >= lastBarTime) { lastBarTime = last.time; formingRaw = { ...last }; drawCandle(id, last); }
}

// ---- Список инструментов --------------------------------------------------
async function refreshInstruments() {
  const r = await fetch('/instruments');
  const list = await r.json().catch(() => []);
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
    const [b, q] = it.symbol.split('-');
    el.innerHTML =
      `<div class="sym">${b}<small>/${q || 'USDT'}</small></div>` +
      `<div class="chg ta-r ${up ? 'up' : 'down'}">${up ? '+' : ''}${it.change.toFixed(2)}%</div>` +
      `<div class="sell ta-r">${it.bid != null ? fmtP(it.id, it.bid) : '—'}</div>` +
      `<div class="buy ta-r">${it.ask != null ? fmtP(it.id, it.ask) : '—'}</div>` +
      `<div class="ta-r muted">${it.bid != null && it.ask != null ? (it.ask - it.bid) : '—'}</div>`;
    el.classList.toggle('active', it.id === selected);
  }
  updateDealPanel();
  pollAccount();
}

// ---- Панель сделки --------------------------------------------------------
function updateDealPanel() {
  const it = META[selected]; if (!it) return;
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

// ---- Выбор инструмента / таймфрейма ---------------------------------------
async function selectInstrument(id) {
  selected = id;
  Object.entries(rowEls).forEach(([k, el]) => el.classList.toggle('active', +k === id));
  document.getElementById('price').value = '';
  await loadCandles(id, tf);
  updateDealPanel();
  pollAccount();
}

document.getElementById('tfs').addEventListener('click', (e) => {
  const btn = e.target.closest('button'); if (!btn) return;
  tf = +btn.dataset.tf;
  document.querySelectorAll('#tfs button').forEach((b) => b.classList.toggle('active', b === btn));
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
  i.value = Math.max(0.001, (parseFloat(i.value) || 0) + d * 0.01).toFixed(3);
}

// Открыть бумажную сделку по рыночной цене (broker-режим, ADR-014).
async function openDeal(side) {
  const id = selected; if (id == null) return;
  const lot = parseFloat(document.getElementById('lot').value) || 0;
  const msg = document.getElementById('dealMsg');
  if (lot <= 0) { msg.textContent = 'Укажи объём'; msg.style.color = 'var(--down)'; return; }
  const body = { instrument: id, side, qty: Math.round(lot * qscale(id)) };
  const r = await fetch('/deals', {
    method: 'POST', headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token()}` },
    body: JSON.stringify(body),
  });
  const data = await r.json().catch(() => null);
  if (r.ok) {
    msg.textContent = `Открыта #${data.id}: ${side === 'buy' ? 'BUY' : 'SELL'} ${lot} @ ${fmtP(id, data.entry)}`;
    msg.style.color = 'var(--up)';
    switchBottom('deals');
    pollAccount(); pollDeals();
  } else {
    msg.textContent = `Отказ ${r.status}: ${data?.error || ''}`;
    msg.style.color = 'var(--down)';
  }
}
document.getElementById('btnBuy').addEventListener('click', () => openDeal('buy'));
document.getElementById('btnSell').addEventListener('click', () => openDeal('sell'));
document.getElementById('userSel').addEventListener('change', () => { pollAccount(); pollDeals(); });

// ---- Счёт (метрики) + позиции ---------------------------------------------
const usd = (cents) => '$' + (cents / 100).toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });

async function pollAccount() {
  try {
    const r = await fetch('/account', { headers: { Authorization: `Bearer ${token()}` } });
    if (!r.ok) return;
    const a = await r.json();
    document.getElementById('mBalance').textContent = usd(a.balance);
    document.getElementById('mEquity').textContent = usd(a.equity);
    document.getElementById('mMargin').textContent = usd(a.used_margin);
    document.getElementById('mFree').textContent = usd(a.free_margin);
    const pnl = document.getElementById('mPnl');
    pnl.textContent = usd(a.open_pnl);
    pnl.style.color = a.open_pnl > 0 ? 'var(--up)' : a.open_pnl < 0 ? 'var(--down)' : '';
  } catch (e) { /* игнор */ }
}

async function pollDeals() {
  try {
    const r = await fetch('/deals', { headers: { Authorization: `Bearer ${token()}` } });
    if (!r.ok) return;
    const list = await r.json();
    const cont = document.getElementById('deals');
    cont.innerHTML = list.map((d) => {
      const pos = d.pnl >= 0;
      const lot = (d.qty / 10 ** d.qty_decimals).toFixed(d.qty_decimals);
      const f = (raw) => (raw / 10 ** d.price_decimals).toFixed(d.price_decimals);
      return `<div class="deal-row"><span>${d.symbol}</span>` +
        `<span class="sd ${d.side}">${d.side === 'buy' ? 'BUY' : 'SELL'}</span>` +
        `<span>${lot}</span><span>${f(d.entry)}</span><span>${f(d.mark)}</span>` +
        `<span class="pnl ${pos ? 'pos' : 'neg'}">${usd(d.pnl)}</span>` +
        `<button class="closebtn" data-id="${d.id}">Close</button></div>`;
    }).join('') || '<div class="deal-row muted"><span>нет открытых позиций</span></div>';
    cont.querySelectorAll('.closebtn').forEach((btn) =>
      btn.addEventListener('click', () => closeDeal(+btn.dataset.id)));
  } catch (e) { /* игнор */ }
}

async function closeDeal(id) {
  const r = await fetch(`/deals/${id}/close`, { method: 'POST', headers: { Authorization: `Bearer ${token()}` } });
  if (r.ok) { pollAccount(); pollDeals(); }
}

// ---- Переключение нижних вкладок ------------------------------------------
function switchBottom(which) {
  document.getElementById('tabTrades').classList.toggle('active', which === 'trades');
  document.getElementById('tabDeals').classList.toggle('active', which === 'deals');
  document.getElementById('paneTrades').classList.toggle('hidden', which !== 'trades');
  document.getElementById('paneDeals').classList.toggle('hidden', which !== 'deals');
}
document.getElementById('tabTrades').addEventListener('click', () => switchBottom('trades'));
document.getElementById('tabDeals').addEventListener('click', () => switchBottom('deals'));

// ---- Поиск ----------------------------------------------------------------
document.getElementById('search').addEventListener('input', (e) => {
  const q = e.target.value.toLowerCase();
  Object.entries(rowEls).forEach(([id, el]) => {
    el.style.display = (META[id]?.symbol || '').toLowerCase().includes(q) ? '' : 'none';
  });
});

// ---- Старт ----------------------------------------------------------------
refreshInstruments();
setInterval(refreshInstruments, 1000); // список слева
setInterval(syncLast, 2000);           // сверка последней свечи (без сброса зума)
setInterval(pollAccount, 1000);        // метрики счёта (equity/P&L)
setInterval(pollDeals, 1200);          // открытые позиции + live P&L
connectWS();
pollAccount();
pollDeals();
