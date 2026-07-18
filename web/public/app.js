'use strict';

// ============================ State ========================================
let META = {}, selected = null, tf = 3600, orderMode = 'market';
let chartType = 'candles', priceType = 'mid', showPips = false, showSLTP = true;
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

// ============================ Indicators (math) ============================
const HL2 = (c) => (c.high + c.low) / 2;
const HLC3 = (c) => (c.high + c.low + c.close) / 3;
const toSeries = (arr, t) => arr.map((v, i) => (v == null || !isFinite(v) ? null : { time: t[i], value: v })).filter(Boolean);

function sma(v, p) { const o = []; let s = 0; for (let i = 0; i < v.length; i++) { s += v[i]; if (i >= p) s -= v[i - p]; o.push(i >= p - 1 ? s / p : null); } return o; }
function ema(v, p) { const o = []; const k = 2 / (p + 1); let e; for (let i = 0; i < v.length; i++) { e = i === 0 ? v[0] : v[i] * k + e * (1 - k); o.push(i >= p - 1 ? e : null); } return o; }
function wma(v, p) { const o = new Array(v.length).fill(null); const d = p * (p + 1) / 2; for (let i = p - 1; i < v.length; i++) { let s = 0; for (let j = 0; j < p; j++) s += v[i - j] * (p - j); o[i] = s / d; } return o; }
function smma(v, p) { const o = new Array(v.length).fill(null); let a; for (let i = 0; i < v.length; i++) { if (i < p) { if (i === p - 1) { let s = 0; for (let j = 0; j < p; j++) s += v[j]; a = s / p; o[i] = a; } } else { a = (a * (p - 1) + v[i]) / p; o[i] = a; } } return o; }
function stdev(v, p) { const o = new Array(v.length).fill(null); for (let i = p - 1; i < v.length; i++) { let m = 0; for (let j = i - p + 1; j <= i; j++) m += v[j]; m /= p; let s = 0; for (let j = i - p + 1; j <= i; j++) s += (v[j] - m) ** 2; o[i] = Math.sqrt(s / p); } return o; }
function trueRange(cs) { const t = []; for (let i = 0; i < cs.length; i++) t.push(i === 0 ? cs[0].high - cs[0].low : Math.max(cs[i].high - cs[i].low, Math.abs(cs[i].high - cs[i - 1].close), Math.abs(cs[i].low - cs[i - 1].close))); return t; }
function atr(cs, p) { const tr = trueRange(cs), o = new Array(cs.length).fill(null); let a; for (let i = 0; i < cs.length; i++) { if (i < p) { if (i === p - 1) { let s = 0; for (let j = 0; j < p; j++) s += tr[j]; a = s / p; o[i] = a; } } else { a = (a * (p - 1) + tr[i]) / p; o[i] = a; } } return o; }
function rsi(v, p) { const o = new Array(v.length).fill(null); let ag = 0, al = 0; for (let i = 1; i < v.length; i++) { const c = v[i] - v[i - 1], g = Math.max(c, 0), l = Math.max(-c, 0); if (i <= p) { ag += g; al += l; if (i === p) { ag /= p; al /= p; o[i] = 100 - 100 / (1 + (al === 0 ? 100 : ag / al)); } } else { ag = (ag * (p - 1) + g) / p; al = (al * (p - 1) + l) / p; o[i] = 100 - 100 / (1 + (al === 0 ? 100 : ag / al)); } } return o; }
function highest(cs, p, i) { let h = -Infinity; for (let j = i - p + 1; j <= i; j++) h = Math.max(h, cs[j].high); return h; }
function lowest(cs, p, i) { let l = Infinity; for (let j = i - p + 1; j <= i; j++) l = Math.min(l, cs[j].low); return l; }
function stoch(cs, kP, dP) { const k = new Array(cs.length).fill(null); for (let i = kP - 1; i < cs.length; i++) { const h = highest(cs, kP, i), l = lowest(cs, kP, i); k[i] = h === l ? 50 : 100 * (cs[i].close - l) / (h - l); } const d = new Array(cs.length).fill(null); for (let i = kP - 1 + dP - 1; i < cs.length; i++) { let s = 0; for (let j = 0; j < dP; j++) s += k[i - j]; d[i] = s / dP; } return [k, d]; }
function cci(cs, p) { const tp = cs.map(HLC3), o = new Array(cs.length).fill(null); for (let i = p - 1; i < cs.length; i++) { let m = 0; for (let j = i - p + 1; j <= i; j++) m += tp[j]; m /= p; let md = 0; for (let j = i - p + 1; j <= i; j++) md += Math.abs(tp[j] - m); md /= p; o[i] = md === 0 ? 0 : (tp[i] - m) / (0.015 * md); } return o; }
function momentum(v, p) { const o = new Array(v.length).fill(null); for (let i = p; i < v.length; i++) o[i] = v[i] - v[i - p]; return o; }
function roc(v, p) { const o = new Array(v.length).fill(null); for (let i = p; i < v.length; i++) o[i] = v[i - p] ? 100 * (v[i] - v[i - p]) / v[i - p] : 0; return o; }
function williams(cs, p) { const o = new Array(cs.length).fill(null); for (let i = p - 1; i < cs.length; i++) { const h = highest(cs, p, i), l = lowest(cs, p, i); o[i] = h === l ? -50 : -100 * (h - cs[i].close) / (h - l); } return o; }
function ao(cs) { const m = cs.map(HL2); const f = sma(m, 5), s = sma(m, 34); return m.map((_, i) => (f[i] != null && s[i] != null ? f[i] - s[i] : null)); }
function acOsc(cs) { const a = ao(cs); const s = sma(a.map((x) => x ?? 0), 5); return a.map((x, i) => (x != null && s[i] != null ? x - s[i] : null)); }
function obv(cs) { const o = new Array(cs.length).fill(null); let v = 0; o[0] = 0; for (let i = 1; i < cs.length; i++) { v += Math.sign(cs[i].close - cs[i - 1].close) * cs[i].volume; o[i] = v; } return o; }
function adLine(cs) { const o = new Array(cs.length).fill(null); let v = 0; for (let i = 0; i < cs.length; i++) { const r = cs[i].high - cs[i].low; v += r ? ((cs[i].close - cs[i].low) - (cs[i].high - cs[i].close)) / r * cs[i].volume : 0; o[i] = v; } return o; }
function cmf(cs, p) { const o = new Array(cs.length).fill(null); for (let i = p - 1; i < cs.length; i++) { let mfv = 0, vol = 0; for (let j = i - p + 1; j <= i; j++) { const r = cs[j].high - cs[j].low; mfv += r ? ((cs[j].close - cs[j].low) - (cs[j].high - cs[j].close)) / r * cs[j].volume : 0; vol += cs[j].volume; } o[i] = vol ? mfv / vol : 0; } return o; }
function mfi(cs, p) { const o = new Array(cs.length).fill(null); const tp = cs.map(HLC3); for (let i = p; i < cs.length; i++) { let pos = 0, neg = 0; for (let j = i - p + 1; j <= i; j++) { const rmf = tp[j] * cs[j].volume; if (tp[j] > tp[j - 1]) pos += rmf; else if (tp[j] < tp[j - 1]) neg += rmf; } o[i] = neg === 0 ? 100 : 100 - 100 / (1 + pos / neg); } return o; }
function forceIndex(cs, p) { const f = new Array(cs.length).fill(null); for (let i = 1; i < cs.length; i++) f[i] = (cs[i].close - cs[i - 1].close) * cs[i].volume; const e = ema(f.map((x) => x ?? 0), p); return e.map((x, i) => (f[i] == null ? null : x)); }
function macdFull(v) { const e12 = ema(v, 12), e26 = ema(v, 26); const line = v.map((_, i) => (e12[i] != null && e26[i] != null ? e12[i] - e26[i] : null)); const sig = ema(line.map((x) => x ?? 0), 9).map((x, i) => (line[i] == null ? null : x)); return [line, sig]; }
function adxDI(cs, p) {
  const n = cs.length, tr = [], pdm = [], ndm = [];
  for (let i = 0; i < n; i++) { if (i === 0) { tr.push(0); pdm.push(0); ndm.push(0); continue; } const up = cs[i].high - cs[i - 1].high, dn = cs[i - 1].low - cs[i].low; pdm.push(up > dn && up > 0 ? up : 0); ndm.push(dn > up && dn > 0 ? dn : 0); tr.push(Math.max(cs[i].high - cs[i].low, Math.abs(cs[i].high - cs[i - 1].close), Math.abs(cs[i].low - cs[i - 1].close))); }
  const pdi = new Array(n).fill(null), ndi = new Array(n).fill(null), adx = new Array(n).fill(null), dx = new Array(n).fill(null);
  let atrS = 0, pS = 0, nS = 0;
  for (let i = 1; i < n; i++) { if (i <= p) { atrS += tr[i]; pS += pdm[i]; nS += ndm[i]; if (i === p) { pdi[i] = 100 * pS / (atrS || 1); ndi[i] = 100 * nS / (atrS || 1); dx[i] = 100 * Math.abs(pdi[i] - ndi[i]) / (pdi[i] + ndi[i] || 1); } } else { atrS = atrS - atrS / p + tr[i]; pS = pS - pS / p + pdm[i]; nS = nS - nS / p + ndm[i]; pdi[i] = 100 * pS / (atrS || 1); ndi[i] = 100 * nS / (atrS || 1); dx[i] = 100 * Math.abs(pdi[i] - ndi[i]) / (pdi[i] + ndi[i] || 1); } }
  let a, cnt = 0, sum = 0; for (let i = 1; i < n; i++) { if (dx[i] == null) continue; cnt++; if (cnt <= p) { sum += dx[i]; if (cnt === p) { a = sum / p; adx[i] = a; } } else { a = (a * (p - 1) + dx[i]) / p; adx[i] = a; } }
  return [adx, pdi, ndi];
}
function bollinger(cs) { const c = cs.map((x) => x.close); const mid = sma(c, 20), sd = stdev(c, 20); return [{ color: 'rgba(120,144,156,.8)', values: mid.map((m, i) => (m != null && sd[i] != null ? m + 2 * sd[i] : null)) }, { color: 'rgba(120,144,156,.4)', values: mid }, { color: 'rgba(120,144,156,.8)', values: mid.map((m, i) => (m != null && sd[i] != null ? m - 2 * sd[i] : null)) }]; }
function keltner(cs) { const c = cs.map((x) => x.close); const mid = ema(c, 20), a = atr(cs, 20); return [{ color: 'rgba(255,183,77,.8)', values: mid.map((m, i) => (m != null && a[i] != null ? m + 2 * a[i] : null)) }, { color: 'rgba(255,183,77,.4)', values: mid }, { color: 'rgba(255,183,77,.8)', values: mid.map((m, i) => (m != null && a[i] != null ? m - 2 * a[i] : null)) }]; }
function donchian(cs) { const u = new Array(cs.length).fill(null), l = new Array(cs.length).fill(null), m = new Array(cs.length).fill(null); for (let i = 19; i < cs.length; i++) { u[i] = highest(cs, 20, i); l[i] = lowest(cs, 20, i); m[i] = (u[i] + l[i]) / 2; } return [{ color: 'rgba(79,195,247,.8)', values: u }, { color: 'rgba(79,195,247,.4)', values: m }, { color: 'rgba(79,195,247,.8)', values: l }]; }
function superTrend(cs, p = 10, mult = 3) { const a = atr(cs, p), st = new Array(cs.length).fill(null); let trend = 1, fu = null, fd = null; for (let i = 1; i < cs.length; i++) { if (a[i] == null) continue; const hl = HL2(cs[i]); const bu = hl + mult * a[i], bl = hl - mult * a[i]; fu = (fu == null || bu < fu || cs[i - 1].close > fu) ? bu : fu; fd = (fd == null || bl > fd || cs[i - 1].close < fd) ? bl : fd; if (trend === 1) { if (cs[i].close < fd) trend = -1; } else { if (cs[i].close > fu) trend = 1; } st[i] = trend === 1 ? fd : fu; } return [{ color: '#26a69a', values: st }]; }
function psar(cs, step = 0.02, max = 0.2) { const o = new Array(cs.length).fill(null); if (cs.length < 2) return [{ color: '#9aa7b5', values: o }]; let bull = true, af = step, ep = cs[0].high, sar = cs[0].low; for (let i = 1; i < cs.length; i++) { sar = sar + af * (ep - sar); if (bull) { if (cs[i].low < sar) { bull = false; sar = ep; ep = cs[i].low; af = step; } else if (cs[i].high > ep) { ep = cs[i].high; af = Math.min(af + step, max); } } else { if (cs[i].high > sar) { bull = true; sar = ep; ep = cs[i].high; af = step; } else if (cs[i].low < ep) { ep = cs[i].low; af = Math.min(af + step, max); } } o[i] = sar; } return [{ color: '#e0e0e0', values: o }]; }
function ichimoku(cs) { const n = cs.length, ten = new Array(n).fill(null), kij = new Array(n).fill(null), sa = new Array(n).fill(null), sb = new Array(n).fill(null); for (let i = 0; i < n; i++) { if (i >= 8) ten[i] = (highest(cs, 9, i) + lowest(cs, 9, i)) / 2; if (i >= 25) kij[i] = (highest(cs, 26, i) + lowest(cs, 26, i)) / 2; if (ten[i] != null && kij[i] != null) sa[i] = (ten[i] + kij[i]) / 2; if (i >= 51) sb[i] = (highest(cs, 52, i) + lowest(cs, 52, i)) / 2; } return [{ color: '#42a5f5', values: ten }, { color: '#ef5350', values: kij }, { color: 'rgba(38,166,154,.6)', values: sa }, { color: 'rgba(239,83,80,.6)', values: sb }]; }
function vwap(cs) { const o = []; let cpv = 0, cv = 0; for (let i = 0; i < cs.length; i++) { cpv += HLC3(cs[i]) * cs[i].volume; cv += cs[i].volume; o.push(cv > 0 ? cpv / cv : null); } return [{ color: '#f0b90b', values: o, width: 2 }]; }
function alligator(cs) { const m = cs.map(HL2); return [{ color: '#2196f3', values: smma(m, 13) }, { color: '#ef5350', values: smma(m, 8) }, { color: '#26a69a', values: smma(m, 5) }]; }

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

let mainSeries = null, overlaySeries = [], oscSeriesList = [], priceLines = [];
let lastBarTime = 0, formingRaw = null;
const volColor = (c) => (c.close >= c.open ? 'rgba(38,166,154,.4)' : 'rgba(239,83,80,.4)');
const isLineType = () => (chartType === 'line' || chartType === 'area');

function heikin(cs) { const o = []; let po, pc; for (let i = 0; i < cs.length; i++) { const c = cs[i]; const hc = (c.open + c.high + c.low + c.close) / 4; const ho = i === 0 ? (c.open + c.close) / 2 : (po + pc) / 2; o.push({ time: c.time, open: ho, high: Math.max(c.high, ho, hc), low: Math.min(c.low, ho, hc), close: hc }); po = ho; pc = hc; } return o; }
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
}
async function selectInstrument(id) { selected = id; Object.entries(rowEls).forEach(([k, el]) => el.classList.toggle('active', +k === id)); document.getElementById('price').value = ''; document.getElementById('sl').value = ''; document.getElementById('tp').value = ''; await loadCandles(id, tf); updateDealPanel(); pollAccount(); pollDeals(); }
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
function stepLot(d) { const i = document.getElementById('lot'); i.value = Math.max(0.001, (parseFloat(i.value) || 0) + d * 0.01).toFixed(3); }
function readSlTp(id) { const sl = parseFloat(document.getElementById('sl').value), tp = parseFloat(document.getElementById('tp').value); return { sl: sl > 0 ? Math.round(sl * pscale(id)) : null, tp: tp > 0 ? Math.round(tp * pscale(id)) : null }; }
async function submit(side) {
  const id = selected; if (id == null) return; const lot = parseFloat(document.getElementById('lot').value) || 0; const msg = document.getElementById('dealMsg');
  if (lot <= 0) { msg.textContent = 'Enter volume'; msg.style.color = 'var(--down)'; return; }
  const { sl, tp } = readSlTp(id); const qty = Math.round(lot * qscale(id)); let url, body;
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
