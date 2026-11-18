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

module.exports = { openTerminal, startServer, findBrowser };
