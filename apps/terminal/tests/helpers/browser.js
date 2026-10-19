'use strict';
// ============================================================================
// Браузерный харнесс терминала (TRM-18).
//
// ЗАЧЕМ. Три проверки упирались в одно и то же отсутствующее средство, и
// поодиночке к ним решено не возвращаться:
//   1) `drawings` — рисует на <canvas>, а jsdom без нативного пакета `canvas`
//      возвращает `null` на getContext('2d');
//   2) postMessage-половина контракта SSO — живёт между iframe и родителем;
//   3) видимость метки устаревания — это CSS и раскладка, которых у jsdom нет.
// Плюс четвёртое, которого не было ни в одном списке: **открывается ли
// приложение вообще**. Зелёный набор модульных тестов этого не доказывает — у
// соседей один клиентский импорт уронил всю админку при 1282 зелёных тестах.
//
// ЧЕМ. `puppeteer-core` + **уже установленный в системе** Chrome или Edge:
// ни одного скачивания браузера на 300 МБ. Тяжёлый харнесс перестают гонять, и
// тогда он не защищает ни от чего. Зависимости — только для разработки
// (Р-020, «Разрешаем только для разработки»), в поставку не входят: образ
// собирается из `public/` как есть.
//
// ГРАНИЦА, И ОНА ВАЖНЕЕ ВОЗМОЖНОСТЕЙ.
// Здесь поднимается **заглушка шлюза**, а не шлюз. Всё, что этот харнесс
// доказывает, — про ФРОНТ: что страница открывается, что виджеты рисуют то, что
// им дали, что сообщения между iframe и родителем ходят. Он не доказывает
// **ничего** про gateway, про Binance и про фид. Скриншот локального интерфейса
// с котировками свидетельством работы фида не является — это записано в
// правилах, и заглушка здесь именно тот случай, ради которого записано.
//
// Отдельно: половина SSO, которую закрывает харнесс, — **наша**. Родитель тут
// поддельный, и согласие настоящего кабинета с контрактом отсюда не следует.
// ============================================================================
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

/**
 * Где взять браузер.
 *
 * Отсутствие браузера — **не повод пропустить проверку молча**. Непройденное
 * обязано быть отличимо от пройденного, поэтому здесь бросается исключение с
 * внятным текстом, а не возвращается `null`.
 */
const CANDIDATES = [
  'C:/Program Files/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Google/Chrome/Application/chrome.exe',
  'C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe',
  'C:/Program Files/Microsoft/Edge/Application/msedge.exe',
  '/usr/bin/google-chrome',
  '/usr/bin/chromium',
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

/** Пустой значок: иконки монет вендорятся в образ, здесь их нет и не нужно. */
const BLANK_SVG = '<svg xmlns="http://www.w3.org/2000/svg" width="1" height="1"></svg>';

/**
 * Заглушка шлюза. Отдаёт правдоподобную форму ответов — ровно чтобы фронт
 * построился, — и НИЧЕГО не утверждает о настоящем шлюзе.
 *
 * `routes` позволяет тесту подменить или сломать отдельную ручку: отказ,
 * который умеет воспроизводить только природа, проверенным не считается, а
 * здесь мы его воспроизводим сами.
 */
const DEPTH = {
  price_decimals: 2,
  qty_decimals: 5,
  // Цены НЕ кратны шагу группировки: иначе корзины совпали бы с уровнями и
  // проверка группировки была бы зелёной при любой формуле.
  bids: [[6500050, 200000], [6500020, 100000], [6499970, 300000]],
  asks: [[6500130, 150000], [6500160, 250000], [6500210, 50000]],
};

const STATS = [{ tf: 60, change: 0.42 }, { tf: 300, change: -1.1 }, { tf: 3600, change: null }];

function stubApi(routes) {
  const candles = () => {
    const out = [];
    let t = Math.floor(Date.now() / 1000) - 300 * 60;
    let p = 65000;
    for (let i = 0; i < 300; i += 1) {
      const open = p;
      // Ряд, который ЕСТЬ ЧЕМ сломать: цена и растёт и падает, объёмы разные,
      // есть разрывы. Вырожденный ряд дал бы зелёное на любой формуле.
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
      { id: 1, symbol: 'BTCUSD', base: 'BTC', quote: 'USD', price_decimals: 2, qty_decimals: 5 },
      { id: 2, symbol: 'ETHUSD', base: 'ETH', quote: 'USD', price_decimals: 2, qty_decimals: 5 },
    ],
    '/candles/1': candles,
    '/candles/2': candles,
    '/depth/1': () => DEPTH,
    '/depth/2': () => DEPTH,
    '/stats/1': () => STATS,
    '/stats/2': () => STATS,
    '/account': () => ({
      balance: 1000000, equity: 1001234, used_margin: 20000, free_margin: 981234,
      open_pnl: 1234, model: 'netting',
    }),
    '/deals': () => [],
    '/deals/closed': () => [],
    '/pending': () => [],
  };
  return Object.assign(base, routes || {});
}

/**
 * Поднимает статику терминала и заглушку шлюза на случайном порту.
 *
 * `config.js` рендерится здесь так же, как его рендерит entrypoint образа из
 * шаблона: иначе харнесс проверял бы страницу, которой в поставке нет.
 */
function startServer(opts) {
  const api = stubApi(opts.routes);
  const cfg = opts.config || {};

  const server = http.createServer((req, res) => {
    const url = new URL(req.url, 'http://localhost');
    const pathname = url.pathname;

    const handler = api[pathname];
    if (handler) {
      const out = handler(url, req);
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
          brandUrl: cfg.brandUrl || '',
          cabinetOrigins: cfg.cabinetOrigins || [''],
        }) + ';\n',
      );
      return;
    }

    if (pathname === '/vendor/lightweight-charts.js') {
      res.writeHead(200, { 'Content-Type': MIME['.js'] });
      res.end(fs.readFileSync(CHARTS));
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

  /**
   * Лента `/stream` — настоящий сокет, а не подделка его состояния.
   *
   * Нужна не ради «зелёной точки»: через неё подаётся **битое** сообщение, и
   * проверяется, что терминал оставляет след, а не глотает его молча (TRM-16).
   * Этот путь модульными тестами не закрывался вовсе — `app.js` в jsdom не
   * поднимается.
   */
  const wss = new WebSocketServer({ server, path: '/stream' });
  const sockets = new Set();
  wss.on('connection', (sock) => {
    sockets.add(sock);
    sock.on('close', () => sockets.delete(sock));
  });
  /** Отправить всем клиентам ровно то, что дали: строку — как есть. */
  const push = (payload) => {
    const text = typeof payload === 'string' ? payload : JSON.stringify(payload);
    for (const s of sockets) s.send(text);
  };

  return new Promise((resolve) => {
    server.listen(0, '127.0.0.1', () => resolve({
      server, port: server.address().port, push, clients: () => sockets.size,
      closeWs: () => new Promise((r) => wss.close(r)),
    }));
  });
}

/**
 * Поднять терминал в настоящем браузере.
 *
 * Возвращает `{ page, origin, errors, cleanup }`. `errors` — всё, что страница
 * сказала в консоль уровнем `error`, плюс необработанные исключения: молчание
 * страницы проверяется так же, как её разметка.
 *
 * Уборка висит на `t.after`, а не в конце тела теста: тест, который падает,
 * иначе оставил бы процесс браузера жить, и прогон завис бы вместо того, чтобы
 * покраснеть.
 */
async function openTerminal(opts) {
  const options = opts || {};
  const { server, port, push, clients, closeWs } = await startServer(options);
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
    try { await browser.close(); } catch { /* уже закрыт */ }
    await closeWs();
    await new Promise((r) => server.close(r));
  };

  const url = origin + (options.path || '/') + (options.query || '');
  await page.goto(url, { waitUntil: 'load', timeout: 30000 });

  return { page, origin, errors, cleanup, browser, push, clients };
}

module.exports = { openTerminal, startServer, findBrowser };
