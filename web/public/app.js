'use strict';

// ---- Конфиг (демо-инструмент BTC-USDT) -----------------------------------
const INSTRUMENT = 1;
const PRICE_SCALE = 100;   // price_decimals = 2
const QTY_SCALE = 1000;    // qty_decimals = 3
const TOKENS = { alice: 'alice-token', bob: 'bob-token' };

const px = (raw) => raw / PRICE_SCALE;
const qy = (raw) => raw / QTY_SCALE;
const fmtP = (raw) => px(raw).toLocaleString('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
const fmtQ = (raw) => qy(raw).toFixed(3);

// ---- График ---------------------------------------------------------------
const chartEl = document.getElementById('chart');
const chart = LightweightCharts.createChart(chartEl, {
  layout: { background: { color: '#0b0e14' }, textColor: '#6b7688' },
  grid: { vertLines: { color: 'rgba(31,39,53,.5)' }, horzLines: { color: 'rgba(31,39,53,.5)' } },
  rightPriceScale: { borderColor: '#1f2735' },
  timeScale: { borderColor: '#1f2735', timeVisible: true, secondsVisible: true },
});
const candles = chart.addCandlestickSeries({
  upColor: '#26a69a', downColor: '#ef5350', borderUpColor: '#26a69a',
  borderDownColor: '#ef5350', wickUpColor: '#26a69a', wickDownColor: '#ef5350',
  priceFormat: { type: 'price', precision: 2, minMove: 0.01 },
});
const volume = chart.addHistogramSeries({ priceFormat: { type: 'volume' }, priceScaleId: '', color: '#2b3648' });
volume.priceScale().applyOptions({ scaleMargins: { top: 0.85, bottom: 0 } });
new ResizeObserver(() => chart.applyOptions({ width: chartEl.clientWidth, height: chartEl.clientHeight })).observe(chartEl);

let curBar = null; // {time, open, high, low, close}
let curVol = 0;

function onTrade(rawPrice, rawQty) {
  const price = px(rawPrice);
  const t = Math.floor(Date.now() / 1000);
  if (!curBar || curBar.time !== t) {
    curBar = { time: t, open: price, high: price, low: price, close: price };
    curVol = qy(rawQty);
  } else {
    curBar.high = Math.max(curBar.high, price);
    curBar.low = Math.min(curBar.low, price);
    curBar.close = price;
    curVol += qy(rawQty);
  }
  candles.update(curBar);
  volume.update({ time: t, value: curVol, color: curBar.close >= curBar.open ? 'rgba(38,166,154,.4)' : 'rgba(239,83,80,.4)' });
  document.getElementById('lastPrice').textContent = fmtP(rawPrice);
}

// ---- Стакан (опрос) -------------------------------------------------------
async function refreshBook() {
  try {
    const r = await fetch(`/book/${INSTRUMENT}?depth=12`);
    const b = await r.json();
    const maxQ = Math.max(1, ...b.bids.map((l) => l.qty), ...b.asks.map((l) => l.qty));
    const row = (l, cls) =>
      `<div class="lvl ${cls}"><div class="bar" style="width:${(l.qty / maxQ) * 100}%"></div>` +
      `<span class="p">${fmtP(l.price)}</span><span>${fmtQ(l.qty)}</span></div>`;
    document.getElementById('asks').innerHTML = b.asks.slice().reverse().map((l) => row(l, 'ask')).join('');
    document.getElementById('bids').innerHTML = b.bids.map((l) => row(l, 'bid')).join('');
    const bestBid = b.bids[0]?.price, bestAsk = b.asks[0]?.price;
    document.getElementById('spread').textContent =
      bestBid && bestAsk ? `спред ${fmtP(bestAsk - bestBid)}` : '—';
  } catch (e) { /* игнор */ }
}
setInterval(refreshBook, 700);
refreshBook();

// ---- Лента сделок ---------------------------------------------------------
function addTrade(rawPrice, rawQty, side) {
  const el = document.getElementById('trades');
  const div = document.createElement('div');
  div.className = 'trade';
  const time = new Date().toLocaleTimeString('ru-RU', { hour12: false });
  div.innerHTML = `<span class="p ${side}">${fmtP(rawPrice)}</span><span>${fmtQ(rawQty)}</span><span class="t">${time}</span>`;
  el.prepend(div);
  while (el.childElementCount > 40) el.removeChild(el.lastChild);
}

// ---- WebSocket ------------------------------------------------------------
function connectWS() {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws';
  const ws = new WebSocket(`${proto}://${location.host}/stream`);
  const st = document.getElementById('status');
  ws.onopen = () => { st.textContent = '● live'; st.className = 'status status--on'; };
  ws.onclose = () => { st.textContent = '● offline'; st.className = 'status status--off'; setTimeout(connectWS, 1500); };
  ws.onmessage = (ev) => {
    let events;
    try { events = JSON.parse(ev.data); } catch { return; }
    for (const e of events) {
      if (e.type === 'trade') { onTrade(e.price, e.qty); addTrade(e.price, e.qty, e.taker_side); }
    }
  };
}
connectWS();

// ---- Форма заявки ---------------------------------------------------------
let side = 'buy';
document.querySelectorAll('.side-btn').forEach((btn) => {
  btn.addEventListener('click', () => {
    side = btn.dataset.side;
    document.querySelectorAll('.side-btn').forEach((b) => b.classList.toggle('active', b === btn));
  });
});

async function placeOrder(token, side, otype, priceReal, qtyReal) {
  const body = { instrument: INSTRUMENT, side, type: otype, qty: Math.round(qtyReal * QTY_SCALE) };
  if (otype === 'limit') body.price = Math.round(priceReal * PRICE_SCALE);
  const r = await fetch('/orders', {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', Authorization: `Bearer ${token}` },
    body: JSON.stringify(body),
  });
  return { ok: r.ok, status: r.status, data: await r.json().catch(() => null) };
}

document.getElementById('orderForm').addEventListener('submit', async (e) => {
  e.preventDefault();
  const token = document.getElementById('actor').value;
  const otype = document.getElementById('otype').value;
  const price = parseFloat(document.getElementById('price').value);
  const qty = parseFloat(document.getElementById('qty').value);
  const msg = document.getElementById('formMsg');
  if (!qty || qty <= 0 || (otype === 'limit' && !(price > 0))) { msg.textContent = 'Заполни цену и количество'; return; }
  const res = await placeOrder(token, side, otype, price, qty);
  msg.textContent = res.ok ? `OK: заявка #${res.data.order_id}` : `Отказ ${res.status}: ${res.data?.error || ''}`;
  msg.style.color = res.ok ? 'var(--up)' : 'var(--down)';
  refreshBook();
});

// ---- Авто-демо (alice продаёт, bob покупает — без self-trade) -------------
let autoTimer = null;
let mid = 500; // реальная цена, ~500.00
async function autoTick() {
  mid += (Math.random() - 0.5) * 0.8;
  mid = Math.max(480, Math.min(520, mid));
  const spread = 0.15;
  const q = () => 1 + Math.floor(Math.random() * 3); // 0.001..0.003 в реальных единицах
  const qr = (n) => n / QTY_SCALE;
  try {
    // ликвидность
    await placeOrder(TOKENS.alice, 'sell', 'limit', +(mid + spread).toFixed(2), qr(q()));
    await placeOrder(TOKENS.bob, 'buy', 'limit', +(mid - spread).toFixed(2), qr(q()));
    // сделка
    if (Math.random() < 0.5) await placeOrder(TOKENS.bob, 'buy', 'market', 0, qr(q()));
    else await placeOrder(TOKENS.alice, 'sell', 'market', 0, qr(q()));
  } catch (e) { /* игнор */ }
}
document.getElementById('autoBtn').addEventListener('click', () => {
  const btn = document.getElementById('autoBtn');
  if (autoTimer) {
    clearInterval(autoTimer); autoTimer = null;
    btn.classList.remove('on'); btn.textContent = '▶ Авто-демо';
  } else {
    autoTimer = setInterval(autoTick, 800);
    btn.classList.add('on'); btn.textContent = '⏸ Авто-демо';
  }
});
