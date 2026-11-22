'use strict';
const fs = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const puppeteer = require('puppeteer-core');
const { WebSocketServer } = require('ws');

const APP_DIR = path.join(__dirname, '..', '..');
const PUBLIC_DIR = path.join(APP_DIR, 'public');
const CHARTS = path.join(
  APP_DIR, 'node_modules', 'lightweight-charts', 'dist', 'lightweight-charts.standalone.production.js',
);

const CANDIDATES = [
  'C:/Program Files/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
  'C:/Program Files/Microsoft/Edge/Application/msedge.exe',
  '/usr/bin/google-chrome',
  '/usr/bin/google-chrome-stable',
  '/usr/bin/chromium',
  '/usr/bin/chromium-browser',
  '/snap/bin/chromium',
];

function findBrowser() {
  const explicit = process.env.TERMINAL_TEST_BROWSER;
  if (explicit) {
    if (!fs.existsSync(explicit)) {
      throw new Error(`TERMINAL_TEST_BROWSER указывает на несуществующий файл: ${explicit}`);
    }
    return explicit;
  }
  const found = CANDIDATES.find((p) => fs.existsSync(p));
  if (!found) {
    throw new Error(
      'Браузер не найден — браузерные проверки НЕ ВЫПОЛНЕНЫ (это блокирующий результат, ' +
      'а не успех). Поставь Chrome или Edge либо укажи путь в TERMINAL_TEST_BROWSER. ' +
      `Искал: ${CANDIDATES.join(', ')}`,
    );
  }
  return found;
}

const MIME = {
  '.html': 'text/html; charset=utf-8',
  '.js': 'text/javascript; charset=utf-8',
  '.css': 'text/css; charset=utf-8',
  '.json': 'application/json; charset=utf-8',
  '.svg': 'image/svg+xml',
};

const BLANK_SVG = '<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"></svg>';

const DEPTH = {
  price_decimals: 2,
  qty_decimals: 5,
  bids: [[6500050, 200000], [6500020, 100000], [6499970, 300000]],
  asks: [[6500130, 150000], [6500160, 250000], [6500210, 50000]],
};

const STATS = [{ tf: 60, change: 0.42 }, { tf: 300, change: -1.1 }, { tf: 3600, change: null }];

function stubApi(routes, sessionTtl) {
  const candles = () => {
    const out = [];
    let t = Math.floor(Date.now() / 1000) - 300 * 60;
    let p = 65000;
    for (let i = 0; i < 300; i += 1) {
      const open = p;
      p += Math.sin(i / 7) * 120 + (i % 11 === 0 ? -260 : 70);
      out.push({
        time: t,
        open,
        high: Math.max(open, p) + 40,
        low: Math.min(open, p) - 40,
        close: p,
        volume: 10 + (i % 17),
      });
      t += 60;
    }
    return out;
  };

  const base = {
    '/health': () => ({ storage: 'memory', durable: false, sso: false }),
    '/instruments': () => [
      {
        id: 1, symbol: 'BTC-USDT', base: 'BTC', quote: 'USDT',
        price_decimals: 2, qty_decimals: 5,
        last: 6500000, bid: 6500000, ask: 6500100, high: 6600000, change: 1.5,
      },
      {
        id: 2, symbol: 'ETH-USDT', base: 'ETH', quote: 'USDT',
        price_decimals: 2, qty_decimals: 5,
        last: null, bid: null, ask: null, high: null, change: null,
      },
    ],
    '/candles/1': candles,
    '/candles/2': candles,
    '/depth/1': () => DEPTH,
    '/depth/2': () => DEPTH,
    '/stats/1': () => STATS,
    '/stats/2': () => STATS,
    '/account': () => ({
      opened: true,
      balance: 1000000, equity: 1001234, used_margin: 20000, free_margin: 981234,
      open_pnl: 1234, model: 'netting',
    }),
    '/v1/session': (_u, _r, body) => {
      const got = JSON.parse(body || '{}');
      return { token: `session-for-${got.token}`, expires_in: sessionTtl };
    },
    '/v1/session/logout': () => ({ ok: true }),
    '/deals': () => [],
    '/deals/closed': () => [],
    '/pending': () => [],
  };
  return Object.assign(base, routes || {});
}

const HOUR = 3600 * 1000;
const FOREX_SESSION = { timezone: 'America/New_York', weekly: [{ open: 'sun 17:00', close: 'fri 17:00' }], holidays: ['2026-12-25'] };
const OPEN = { status: 'open', next_open: null, reason: null, tradable: true };

/**
 * @param {number} base
 * @param {number} amp
 * @param {boolean} withVolume
 */
function wave2Candles(base, amp, withVolume) {
  return (/** @type {URL} */ u) => {
    const out = [];
    const tf = Number(u.searchParams.get('tf')) || 60;
    const now = Math.floor(Date.now() / 1000);
    let t = now - (now % tf) - 299 * tf;
    let p = base;
    for (let i = 0; i < 300; i += 1) {
      const open = p;
      p = Math.round(p + Math.sin(i / 7) * amp + (i % 11 === 0 ? -2 * amp : amp / 2));
      out.push({
        time: t, open, high: Math.max(open, p) + Math.round(amp / 3), low: Math.min(open, p) - Math.round(amp / 3), close: p,
        volume: withVolume ? 10 + (i % 17) : null,
      });
      t += tf;
    }
    return out;
  };
}

/** @param {{ nextOpen?: number|null, eurusd?: object, xauusd?: object, aapl?: object, ltc?: object }} [over] */
function wave2Instruments(over) {
  const o = over || {};
  const nextOpen = o.nextOpen === undefined ? Date.now() + 30 * HOUR : o.nextOpen;
  const v2 = {
    tick_size: null, qty_step: null, min_qty: null, contract_size: null, lot_decimals: null,
    has_depth: true, has_trades: true, has_1s: true, mode: 'trade', market: OPEN, session: null,
  };
  return [
    Object.assign({}, v2, {
      id: 1, symbol: 'BTC-USDT', canonical: 'BTCUSD', name: 'Bitcoin', category: 'crypto', base: 1, quote: 100,
      price_decimals: 2, qty_decimals: 5, tick_size: 1, qty_step: 1, min_qty: 1,
      last: 6500000, bid: 6500000, ask: 6500100, high: 6600000, low: 6400000, change: 1.5, volume: 123400000, quote_volume: 80210000000,
    }),
    Object.assign({}, v2, {
      id: 3, symbol: 'EURUSD', canonical: 'EURUSD', name: 'Euro / US Dollar', category: 'forex', base: 3, quote: 100,
      price_decimals: 5, qty_decimals: 0, tick_size: 1, qty_step: 1000, min_qty: 1000, contract_size: '100000', lot_decimals: 2,
      has_depth: false, has_trades: false, has_1s: false, session: FOREX_SESSION,
      market: { status: 'closed', next_open: nextOpen, reason: 'weekend', tradable: false },
      last: 108414, bid: 108412, ask: 108415, high: 108620, low: 108150, change: -0.12, volume: null, quote_volume: null,
    }, o.eurusd || {}),
    Object.assign({}, v2, {
      id: 4, symbol: 'XAUUSD', canonical: 'XAUUSD', name: 'Gold / US Dollar', category: 'metals', base: 4, quote: 100,
      price_decimals: 2, qty_decimals: 2, tick_size: 1, qty_step: 100, min_qty: 100, contract_size: '100', lot_decimals: 2,
      has_depth: false, has_trades: false, has_1s: false,
      session: { timezone: 'America/New_York', weekly: [{ open: 'sun 18:00', close: 'fri 17:00' }], holidays: [] },
      last: 265012, bid: 265000, ask: 265024, high: 266100, low: 263900, change: 0.48, volume: null, quote_volume: null,
    }, o.xauusd || {}),
    Object.assign({}, v2, {
      id: 5, symbol: 'AAPL', canonical: 'AAPL', name: 'Apple Inc.', category: 'stocks', base: 5, quote: 100,
      price_decimals: 2, qty_decimals: 0, tick_size: 1, qty_step: 1, min_qty: 1, contract_size: '1', lot_decimals: 0,
      has_depth: false, has_trades: true, has_1s: false,
      session: { timezone: 'America/New_York', weekly: ['mon', 'tue', 'wed', 'thu', 'fri'].map((d) => ({ open: `${d} 09:30`, close: `${d} 16:00` })), holidays: ['2026-11-26'] },
      last: 22814, bid: 22813, ask: 22815, high: 22990, low: 22650, change: 0.87, volume: 5123400, quote_volume: 116800000000,
    }, o.aapl || {}),
    Object.assign({}, v2, {
      id: 6, symbol: 'LTC-USDT', canonical: 'LTCUSD', name: 'Litecoin', category: 'crypto', base: 6, quote: 100,
      price_decimals: 2, qty_decimals: 3, mode: 'closeOnly',
      last: 8215, bid: 8214, ask: 8216, high: 8400, low: 8010, change: -1.2, volume: 9123000, quote_volume: 74900000,
    }, o.ltc || {}),
  ];
}

/** @param {Parameters<typeof wave2Instruments>[0]} [over] */
function wave2Routes(over) {
  const list = wave2Instruments(over);
  /** @type {Record<string, Function>} */
  const routes = { '/instruments': () => wave2Instruments(over) };
  /** @type {Record<number, [number, number, boolean]>} */
  const shapes = { 1: [6500000, 12000, true], 3: [108300, 30, false], 4: [264000, 120, false], 5: [22700, 15, true], 6: [8200, 12, true] };
  for (const it of list) {
    const [base, amp, vol] = shapes[it.id];
    const candles = wave2Candles(base, amp, vol);
    routes[`/candles/${it.id}`] = (/** @type {URL} */ u) => (u.searchParams.get('tf') === '1' && !it.has_1s ? { __status: 400, body: { error: 'interval_unsupported' } } : candles(u));
    routes[`/depth/${it.id}`] = () => (it.has_depth ? Object.assign({}, DEPTH, { price_decimals: it.price_decimals, qty_decimals: it.qty_decimals }) : { __status: 404, body: { error: 'depth_unavailable' } });
    routes[`/stats/${it.id}`] = () => STATS;
  }
  return routes;
}

function startServer(opts) {
  const api = stubApi(opts.routes, opts.sessionTtl || 900);
  const cfg = opts.config || {};

  const seen = [];

  const state = { origin: null };

  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url, 'http://localhost');
    const pathname = url.pathname;

    let body = '';
    if (req.method !== 'GET' && req.method !== 'HEAD') {
      for await (const chunk of req) body += chunk;
    }
    seen.push({ method: req.method, path: pathname, auth: req.headers.authorization || null, body });

    const handler = api[pathname];
    if (handler) {
      const out = handler(url, req, body);
      if (out && out.__status) {
        res.writeHead(out.__status, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify(out.body ?? null));
        return;
      }
      res.writeHead(200, { 'Content-Type': 'application/json; charset=utf-8' });
      res.end(JSON.stringify(out));
      return;
    }

    if (pathname === '/config.js') {
      res.writeHead(200, { 'Content-Type': MIME['.js'] });
      res.end(
        'window.TERMINAL_CONFIG = ' +
        JSON.stringify({
          brandUrl: (cfg.brandUrl || '').replace('{origin}', state.origin),
          cabinetOrigins: (cfg.cabinetOrigins || ['{origin}'])
            .map((o) => o.replace('{origin}', state.origin)),
        }) + ';\n',
      );
      return;
    }

    if (pathname === '/vendor/lightweight-charts.js') {
      res.writeHead(200, { 'Content-Type': MIME['.js'] });
      res.end(fs.readFileSync(CHARTS));
      return;
    }

    if (pathname === '/favicon.ico') {
      res.writeHead(200, { 'Content-Type': MIME['.svg'] });
      res.end(BLANK_SVG);
      return;
    }

    if (pathname.startsWith('/vendor/coins/')) {
      res.writeHead(200, { 'Content-Type': MIME['.svg'] });
      res.end(BLANK_SVG);
      return;
    }

    if (opts.pages && opts.pages[pathname]) {
      res.writeHead(200, { 'Content-Type': MIME['.html'] });
      res.end(opts.pages[pathname]);
      return;
    }

    const rel = pathname === '/' ? 'index.html' : pathname.replace(/^\/+/, '');
    const file = path.join(PUBLIC_DIR, rel);
    if (!file.startsWith(PUBLIC_DIR) || !fs.existsSync(file) || fs.statSync(file).isDirectory()) {
      res.writeHead(404, { 'Content-Type': 'text/plain' });
      res.end('not found');
      return;
    }
    res.writeHead(200, { 'Content-Type': MIME[path.extname(file)] || 'application/octet-stream' });
    res.end(fs.readFileSync(file));
  });

  const wss = new WebSocketServer({ server, path: '/stream' });
  const sockets = new Set();
  wss.on('connection', (sock) => {
    sockets.add(sock);
    sock.on('close', () => sockets.delete(sock));
  });
  const push = (payload) => {
    const text = typeof payload === 'string' ? payload : JSON.stringify(payload);
    for (const s of sockets) s.send(text);
  };

  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => {
      state.origin = `http://127.0.0.1:${/** @type {import('node:net').AddressInfo} */ (server.address()).port}`;
      resolve({
        server, port: /** @type {import('node:net').AddressInfo} */ (server.address()).port, push, clients: () => sockets.size, seen,
        closeWs: () => new Promise((r) => wss.close(r)),
      });
    });
  });
}

async function openTerminal(opts) {
  const options = opts || {};
  const { server, port, push, clients, closeWs, seen } = await startServer(options);
  const origin = `http://127.0.0.1:${port}`;

  const browser = await puppeteer.launch({
    executablePath: findBrowser(),
    headless: true,
    args: ['--no-sandbox', '--disable-dev-shm-usage', '--window-size=1400,900'],
  });

  const page = await browser.newPage();
  await page.setViewport({ width: 1400, height: 900 });

  const errors = [];
  page.on('console', (m) => { if (m.type() === 'error') errors.push(m.text()); });
  page.on('pageerror', (e) => errors.push(`pageerror: ${e.message}`));

  const cleanup = async () => {
    try { await browser.close(); } catch {  }
    await closeWs();
    await new Promise((r) => server.close(r));
  };

  const url = origin + (options.path || '/') + (options.query || '');
  await page.goto(url, { waitUntil: 'load', timeout: 30000 });

  return { page, origin, errors, cleanup, browser, push, clients, seen };
}

module.exports = { openTerminal, startServer, findBrowser, wave2Routes, wave2Instruments };
