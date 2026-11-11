'use strict';
// ============================================================================
// i18n.js — локализация терминала RU/EN без внешних библиотек (ADR-015).
//
// Как устроено:
//   • словарь — плоская таблица `ключ → [en, ru]`, одна на весь терминал: её
//     видно целиком, и недостающий перевод находится тестом, а не человеком;
//   • статичная разметка помечается `data-i18n="ключ"` (текст),
//     `data-i18n-title`, `data-i18n-tip`, `data-i18n-ph` (placeholder) —
//     `apply(root)` проходит по ним при старте и при смене языка;
//   • динамический текст берёт `t(ключ, vars)` в момент отрисовки — виджеты
//     перерисовываются опросом, поэтому новый язык доезжает до них сам;
//   • виджеты (ADR-015) про этот модуль не знают: хост даёт им `ctx.t`.
//
// Язык по умолчанию — язык кабинета, если он передан (`?lang=` или `#lang=`
// во входной ссылке), иначе сохранённый выбор человека, иначе RU. Кабинет
// главнее сохранённого: терминал открыт из кабинета, и язык у них обязан
// совпадать — расхождение выглядело бы как два разных продукта.
// ============================================================================
(function () {
  const LANGS = ['ru', 'en'];
  const KEY = 'terminal:lang';

  /** @type {Record<string, [string, string]>} ключ → [en, ru] */
  const D = {
    // ---- Шапка и счёт ----
    'acct.balance': ['Balance', 'Баланс'],
    'acct.credit': ['Credit', 'Кредит'],
    'acct.equity': ['Equity', 'Средства'],
    'acct.margin': ['Margin', 'Маржа'],
    'acct.free': ['Free margin', 'Свободно'],
    'acct.pnl': ['Open P&L', 'Открытый P&L'],
    'acct.level': ['Margin level', 'Уровень маржи'],
    'acct.leverage': ['Leverage', 'Плечо'],
    'acct.leverage.tip': ['Leverage for new positions. Open positions keep the leverage they were opened with.', 'Плечо для новых позиций. Открытые позиции держат плечо, с которым открыты.'],
    'acct.level.tip': ['Equity / used margin. Below the margin-call level you get a warning; at the stop-out level positions are closed automatically.', 'Средства / занятая маржа. Ниже уровня маржин-колла — предупреждение, на уровне стоп-аута позиции закрываются автоматически.'],
    'acct.level.none': ['No open positions — margin level is not defined', 'Позиций нет — уровень маржи не определён'],
    'kind.demo': ['DEMO', 'DEMO'],
    'kind.real': ['REAL ACCOUNT', 'REAL ACCOUNT'],
    'kind.demo.tip': ['Demo account: virtual money, real prices. Switch to your real account in the cabinet.', 'Демо-счёт: виртуальные деньги, реальные цены. Реальный счёт выбирается в кабинете.'],
    'kind.real.tip': ['Real money. Deposits and withdrawals go through your cabinet.', 'Реальные деньги. Пополнение и вывод — в кабинете.'],
    'frozen.badge': ['BLOCKED BY OPERATOR', 'BLOCKED BY OPERATOR'],
    'frozen.tip': ['Opening positions, new orders and withdrawals are disabled. Closing positions is allowed.', 'Открытие позиций, новые ордера и вывод отключены. Закрывать позиции можно.'],
    'frozen.banner': ['Account blocked by operator: opening positions and new orders are disabled. You can still close open positions. Contact support.', 'Счёт заблокирован оператором: открытие позиций и новые ордера недоступны. Закрывать открытые позиции можно. Обратитесь в поддержку.'],
    'frozen.form': ['Trading blocked by operator', 'Торговля заблокирована оператором'],
    'frozen.form.sub': ['Closing positions is still available in the Positions tab', 'Закрыть позиции можно во вкладке «Позиции»'],
    'model.netting': ['NETTING', 'НЕТТИНГ'],
    'model.hedging': ['HEDGING', 'ХЕДЖИРОВАНИЕ'],
    'model.netting.tip': ['Opposite trade reduces or closes your position', 'Встречная сделка уменьшает или закрывает позицию'],
    'model.hedging.tip': ['Opposite trade opens a second, separate position — it does not close this one', 'Встречная сделка открывает вторую, отдельную позицию — эту она не закрывает'],
    'acct.no-session': ['NO CABINET SESSION — TRADING UNAVAILABLE', 'НЕТ СЕССИИ КАБИНЕТА — ТОРГОВЛЯ НЕДОСТУПНА'],
    'acct.unreachable': ['ACCOUNT UNREACHABLE — FIGURES MAY BE STALE', 'СЧЁТ НЕ ОТВЕЧАЕТ — ЦИФРЫ МОГУТ БЫТЬ УСТАРЕВШИМИ'],
    'acct.unknown': ['NO ACCOUNT DATA YET', 'ДАННЫХ СЧЁТА ПОКА НЕТ'],
    'acct.not-opened': ['ACCOUNT OPENS WITH YOUR FIRST TRADE — NO FIGURES YET', 'СЧЁТ ОТКРОЕТСЯ ПЕРВОЙ СДЕЛКОЙ — ЦИФР ПОКА НЕТ'],
    'state.volatile': ['DEMO STATE — NOT SAVED BETWEEN RESTARTS', 'ДЕМО-СОСТОЯНИЕ — НЕ СОХРАНЯЕТСЯ МЕЖДУ ПЕРЕЗАПУСКАМИ'],
    'data.down': ['MARKET DATA UNAVAILABLE: {what}', 'НЕТ РЫНОЧНЫХ ДАННЫХ: {what}'],
    'data.candles': ['candles', 'свечи'],
    'data.depth': ['depth', 'глубина'],
    'data.stats': ['stats', 'статистика'],
    'data.instruments': ['instruments', 'инструменты'],
    'mc.call': ['MARGIN CALL: margin level {lvl}% is below {call}%. At {stop}% positions are closed automatically (stop-out). Add funds or reduce positions.', 'МАРЖИН-КОЛЛ: уровень маржи {lvl}% ниже {call}%. При {stop}% позиции закрываются автоматически (стоп-аут). Пополните счёт или сократите позиции.'],
    'mc.call.nostop': ['MARGIN CALL: margin level {lvl}% is below {call}%. Add funds or reduce positions.', 'МАРЖИН-КОЛЛ: уровень маржи {lvl}% ниже {call}%. Пополните счёт или сократите позиции.'],
    'mc.stopout': ['STOP-OUT: margin level {lvl}% reached the stop-out level {stop}% — positions are being closed.', 'СТОП-АУТ: уровень маржи {lvl}% достиг порога {stop}% — позиции закрываются.'],
    'top.cabinet': ['Cabinet', 'В кабинет'],
    'top.cabinet.tip': ['Back to your cabinet', 'Вернуться в личный кабинет'],
    'top.logout': ['Log out', 'Выйти'],
    'top.theme': ['Light / Dark', 'Светлая / тёмная'],
    'top.lang': ['Language', 'Язык'],
    'feed.live': ['LIVE', 'LIVE'],
    'feed.nodata': ['NO DATA', 'НЕТ ДАННЫХ'],
    'feed.gw': ['no connection to terminal gateway', 'нет связи со шлюзом терминала'],
    'feed.ok': ['market data live', 'рыночные данные идут'],
    'feed.silent': ['socket open, no market data for {age}', 'сокет открыт, рыночных данных нет {age}'],
    'feed.session': ['the whole session', 'всю сессию'],
    // ---- Сессия верхнего уровня ----
    'sso.expiring': ['Your session ends in {min} min.', 'Сессия закончится через {min} мин.'],
    'sso.extend': ['Extend', 'Продлить'],
    'sso.expired': ['Session ended — trading is unavailable.', 'Сессия закончилась — торговля недоступна.'],
    'sso.login': ['Sign in via cabinet', 'Войти через кабинет'],
    'sso.none': ['You are not signed in — trading is unavailable.', 'Вы не вошли — торговля недоступна.'],
    'sso.failed': ['Sign-in link is invalid or expired.', 'Ссылка входа недействительна или устарела.'],
    // ---- Шапка инструмента ----
    'ih.change': ['24h change', 'Изм. 24ч'],
    'ih.high': ['24h high', 'Макс. 24ч'],
    'ih.low': ['24h low', 'Мин. 24ч'],
    'ih.vol': ['24h vol ({c})', 'Объём 24ч ({c})'],
    'ih.spread': ['Spread', 'Спред'],
    'ih.source': ['Prices: Binance', 'Цены: Binance'],
    'ih.source.tip': ['Quotes, chart, depth and market trades come from Binance (spot) through our gateway. Your orders are executed by the broker at these prices (CFD) and do not reach the Binance order book.', 'Котировки, график, глубина и сделки рынка — с Binance (спот) через наш шлюз. Ваши ордера исполняет брокер по этим ценам (CFD); в стакан Binance они не попадают.'],
    'ih.pairs': ['Pairs', 'Пары'],
    // ---- Мобильные вкладки ----
    'm.chart': ['Chart', 'График'],
    'm.book': ['Book', 'Стакан'],
    'm.order': ['Order', 'Ордер'],
    'm.positions': ['Positions', 'Позиции'],
    // ---- График ----
    'tb.chartType': ['Chart type', 'Тип графика'],
    'tb.indicators': ['Indicators', 'Индикаторы'],
    'tb.priceType': ['Price type', 'Тип цены'],
    'tb.sltp': ['Show SL/TP lines', 'Линии SL/TP'],
    'tb.pips': ['Display pips', 'Показывать пункты'],
    'tb.reset': ['Reset chart', 'Сбросить график'],
    'tb.full': ['Fullscreen', 'Во весь экран'],
    'tb.settings': ['Settings', 'Настройки'],
    'ct.candles': ['Candles', 'Свечи'],
    'ct.hollow': ['Hollow candles', 'Полые свечи'],
    'ct.bars': ['Bars', 'Бары'],
    'ct.line': ['Line', 'Линия'],
    'ct.area': ['Area', 'Область'],
    'ct.heikin': ['Heikin Ashi', 'Хейкин-аши'],
    'ind.trend': ['Trend', 'Тренд'],
    'ind.vola': ['Volatility', 'Волатильность'],
    'ind.volume': ['Volume', 'Объём'],
    'ind.osc': ['Oscillator (bottom, one)', 'Осциллятор (внизу, один)'],
    'ind.none': ['None', 'Нет'],
    'ind.soon': ['Soon: Volume Profile, TPO, Delta, Fibonacci, ZigZag, Pivot Points.', 'Скоро: Volume Profile, TPO, Delta, Фибоначчи, ZigZag, Pivot Points.'],
    'legend.forming': ['bar not closed', 'бар не закрыт'],
    'set.candles': ['Candles', 'Свечи'],
    'set.hollow': ['Hollow candle color', 'Цвет полых свечей'],
    'set.shadows': ['Candle shadows', 'Тени свечей'],
    'set.shadowCustom': ['Custom shadow color', 'Свой цвет теней'],
    'set.lines': ['Price lines', 'Ценовые линии'],
    'set.plw': ['Price line width', 'Толщина ценовой линии'],
    'set.addLine': ['Additional price line', 'Доп. ценовая линия'],
    'set.alw': ['Additional line width', 'Толщина доп. линии'],
    'set.minChange': ['Minimal price change', 'Минимальный шаг цены'],
    'set.step': ['Step', 'Шаг'],
    'set.default': ['Default', 'По умолчанию'],
    'set.cols': ['Watchlist columns', 'Колонки списка'],
    // ---- Форма ордера ----
    'f.limit': ['Limit', 'Лимит'],
    'f.market': ['Market', 'Рынок'],
    'f.stop': ['Stop', 'Стоп'],
    'f.stop.tip': ['Stop entry orders are not supported yet. Protective Stop Loss / Take Profit are set in the form.', 'Стоп-ордера на вход пока не поддерживаются. Защитные Stop Loss / Take Profit задаются в форме.'],
    'f.soon': ['soon', 'скоро'],
    'f.price': ['Price', 'Цена'],
    'f.atMarket': ['At market', 'По рынку'],
    'f.size': ['Size', 'Объём'],
    'f.sl': ['Stop Loss', 'Stop Loss'],
    'f.tp': ['Take Profit', 'Take Profit'],
    'f.buy': ['Buy / Long', 'Купить / Long'],
    'f.sell': ['Sell / Short', 'Продать / Short'],
    'f.notional': ['Value', 'Стоимость'],
    'f.marginReq': ['Margin', 'Маржа'],
    'f.free': ['Free', 'Свободно'],
    'f.leverage': ['Leverage', 'Плечо'],
    'f.enterSize': ['Enter size', 'Введите объём'],
    'f.enterPrice': ['Enter price', 'Введите цену'],
    'f.limitPlaced': ['Limit order #{id}', 'Лимитный ордер #{id}'],
    'f.opened': ['Opened #{id}: {side} @ {px}', 'Открыта #{id}: {side} @ {px}'],
    'f.priceUnknown': ['Instrument price is unknown — nothing to convert with', 'Цена инструмента неизвестна — пересчитать нечем'],
    'f.spread': ['Spread: {n} pips', 'Спред: {n} пп.'],
    'f.share.tip': ['Share of free margin at the account leverage', 'Доля свободной маржи с плечом счёта'],
    'fee.text': ['Commission ≈ {v} USD', 'Комиссия ≈ {v} USD'],
    'fee.unknown': ['Commission ≈ — USD', 'Комиссия ≈ — USD'],
    'fee.tip': ['Commission {p}% of the trade notional, charged on opening and again on closing.', 'Комиссия {p}% от стоимости сделки — при открытии и ещё раз при закрытии.'],
    // ---- Нижняя панель ----
    'p.positions': ['Positions', 'Позиции'],
    'p.orders': ['Orders', 'Ордера'],
    'p.history': ['History', 'История'],
    'p.balance': ['Balance', 'Баланс'],
    'p.signals': ['Signals', 'Сигналы'],
    'p.instr': ['Instrument', 'Инструмент'],
    'p.side': ['Side', 'Сторона'],
    'p.qty': ['Size', 'Объём'],
    'p.entry': ['Entry', 'Вход'],
    'p.price': ['Price', 'Цена'],
    'p.mark': ['Current', 'Текущая'],
    'p.exit': ['Exit', 'Выход'],
    'p.pnl': ['P&L', 'P&L'],
    'p.comm': ['Comm.', 'Комис.'],
    'p.signal': ['Signal', 'Сигнал'],
    'p.tf': ['TF', 'ТФ'],
    'p.dir': ['Direction', 'Направление'],
    'p.close': ['Close', 'Закрыть'],
    'p.cancel': ['Cancel', 'Отменить'],
    'p.buy': ['BUY', 'BUY'],
    'p.sell': ['SELL', 'SELL'],
    'p.noPositions': ['No open positions', 'Открытых позиций нет'],
    'p.noOrders': ['No limit orders', 'Лимитных ордеров нет'],
    'p.noHistory': ['No history', 'Истории нет'],
    'p.noSignals': ['No signals on this TF', 'Сигналов на этом ТФ нет'],
    'p.noSession': ['No cabinet session — trading unavailable', 'Нет сессии кабинета — торговля недоступна'],
    'p.signalsNote': ['Our simple indicator-based signals (RSI, MA cross). Not investment advice.', 'Простые сигналы по индикаторам (RSI, пересечение MA). Не инвестиционная рекомендация.'],
    'b.kind': ['Account', 'Счёт'],
    'b.id': ['Account ID', 'ID счёта'],
    'b.model': ['Position model', 'Модель позиций'],
    'b.callAt': ['Margin call at', 'Маржин-колл при'],
    'b.stopAt': ['Stop-out at', 'Стоп-аут при'],
    'b.nbp': ['Negative balance protection', 'Защита от отрицательного баланса'],
    'b.on': ['on', 'включена'],
    'b.off': ['off', 'выключена'],
    'b.markup': ['Spread markup', 'Наценка на спред'],
    'b.commission': ['Commission', 'Комиссия'],
    'b.none': ['not set', 'не задано'],
    'b.status': ['Trading', 'Торговля'],
    'b.active': ['active', 'доступна'],
    'b.blocked': ['blocked by operator', 'заблокирована оператором'],
    'b.noData': ['No account data yet', 'Данных счёта пока нет'],
    // ---- Стакан и сделки ----
    'ob.title': ['Market depth', 'Глубина рынка'],
    'ob.source': ['source: Binance', 'источник: Binance'],
    'ob.source.tip': ['Order book of the price source (Binance spot), not ours: the broker executes your orders at these prices, they do not enter this book.', 'Стакан рынка-источника цен (Binance, спот), а не наш: брокер исполняет ваши ордера по этим ценам, в этот стакан они не попадают.'],
    'ob.price': ['Price', 'Цена'],
    'ob.amount': ['Amount', 'Объём'],
    'ob.total': ['Total', 'Сумма'],
    'ob.spread': ['spread', 'спред'],
    'ob.settings': ['Settings', 'Настройки'],
    'ob.display': ['Order book display', 'Отображение'],
    'ob.avg': ['Show average amount', 'Средний объём'],
    'ob.ratio': ['Show buy/sell ratio', 'Соотношение покупок/продаж'],
    'ob.rounding': ['Rounding', 'Группировка'],
    'ob.depth': ['Visual depth', 'Полосы глубины'],
    'ob.amountMode': ['Amount', 'По объёму'],
    'ob.cum': ['Cumulative', 'Нарастающим итогом'],
    'ob.anim': ['Animation', 'Анимация'],
    'ob.enabled': ['Enabled', 'Включена'],
    'ob.both': ['Bids and asks', 'Покупки и продажи'],
    'ob.bids': ['Bids only', 'Только покупки'],
    'ob.asks': ['Asks only', 'Только продажи'],
    'ob.group': ['Price grouping', 'Шаг группировки'],
    'tr.market': ['Market trades', 'Сделки на рынке'],
    'tr.mine': ['My trades', 'Мои сделки'],
    'tr.price': ['Price', 'Цена'],
    'tr.qty': ['Qty', 'Объём'],
    'tr.time': ['Time', 'Время'],
    'tr.sideQty': ['Side / size', 'Сторона / объём'],
    'tr.entryExit': ['Entry → exit', 'Вход → выход'],
    'tr.waiting': ['Waiting for trades…', 'Ждём сделки…'],
    'tr.none': ['No trades on this instrument', 'Сделок по инструменту нет'],
    'tr.unreadable': ['Could not read trades', 'Не удалось прочитать сделки'],
    'tr.source': ['Binance', 'Binance'],
    'wl.search': ['Search…', 'Поиск…'],
    'wl.title': ['Pairs', 'Пары'],
    'wl.instr': ['Pair', 'Пара'],
    'wl.change': ['Change', 'Изм.'],
    'wl.sell': ['Sell', 'Продажа'],
    'wl.buy': ['Buy', 'Покупка'],
    'wl.spread': ['Spread', 'Спред'],
    'wl.high': ['High', 'Макс.'],
    'wl.price': ['Price', 'Цена'],
    // ---- Выход ----
    'out.title': ['You have signed out of the terminal', 'Вы вышли из терминала'],
    'out.text': ['The terminal session is closed. Your cabinet session is not affected.', 'Сессия терминала закрыта. Сессия кабинета не затронута.'],
    'out.cabinet': ['Back to cabinet', 'Вернуться в кабинет'],
    'out.again': ['Open terminal again', 'Открыть терминал снова'],
  };

  // Отказы границы (rejection.js) — переводы по коду. Английский остаётся в
  // rejection.js: он и есть исходный текст, здесь только русский.
  /** @type {Record<string, string>} */
  const REJECT_RU = {
    instrument_not_allowed: 'Этот инструмент недоступен на этом сайте',
    tenant_instruments_unavailable: 'Разрешения на инструменты не пришли — торговля закрыта. Конфигурация сайта недоступна; это чинится на стороне платформы.',
    tenant_instruments_unreadable: 'В списке инструментов сайта неизвестный символ — торговля закрыта. Список доступен, его нужно поправить в CMS.',
    insufficient_margin: 'Недостаточно свободной маржи',
    stop_loss_already_reached: 'Stop Loss уже по другую сторону цены — позиция закрылась бы сразу. Для покупки ставьте его ниже текущей цены, для продажи — выше.',
    take_profit_already_reached: 'Take Profit уже достигнут — позиция закрылась бы сразу. Для покупки ставьте его выше текущей цены, для продажи — ниже.',
    demo_start_balance_not_set: 'Сайт не задал стартовый баланс демо-счетов, поэтому счёт не открыт. С вашим запросом всё в порядке — сумму задаёт владелец сайта в админке.',
    demo_start_balance_unavailable: 'Настройки сайта недоступны, поэтому счёт не открыт. Запрос в порядке; это сбой на стороне платформы, его чинят.',
    account_frozen: 'Счёт заблокирован оператором: открытие позиций, новые ордера и вывод отключены. Закрывать позиции можно. Обратитесь в поддержку.',
    identity_store_unavailable: 'Не удалось подтвердить, к какому сайту относится счёт, поэтому он не открыт. Запрос в порядке — это сбой на стороне платформы.',
  };

  /** @param {any} v @returns {'ru'|'en'|null} */
  function norm(v) {
    const s = String(v || '').toLowerCase().slice(0, 2);
    return s === 'ru' || s === 'en' ? s : null;
  }

  /**
   * Какой язык взять при старте. Чистая функция — ради тестов.
   * @param {{ query?: string, hash?: string, saved?: string|null, config?: string }} src
   * @returns {'ru'|'en'}
   */
  function detect(src) {
    const q = new URLSearchParams((src.query || '').replace(/^\?/, ''));
    const h = new URLSearchParams((src.hash || '').replace(/^#/, ''));
    return norm(q.get('lang')) || norm(h.get('lang')) || norm(src.config) || norm(src.saved) || 'ru';
  }

  let saved = null;
  try { saved = localStorage.getItem(KEY); } catch { /* приватный режим */ }
  let lang = detect({
    query: typeof location !== 'undefined' ? location.search : '',
    hash: typeof location !== 'undefined' ? location.hash : '',
    saved,
    config: (window.TERMINAL_CONFIG || {}).locale,
  });
  const listeners = [];

  /**
   * @param {string} key
   * @param {Record<string, string|number>} [vars]
   */
  function t(key, vars) {
    const row = D[key];
    let s = row ? row[lang === 'ru' ? 1 : 0] : key;
    if (vars) for (const k of Object.keys(vars)) s = s.split(`{${k}}`).join(String(vars[k]));
    return s;
  }

  /** Есть ли ключ — для виджетов, у которых свой английский по умолчанию. */
  function has(key) { return Object.prototype.hasOwnProperty.call(D, key); }

  /**
   * Перевод для виджета: `(ключ, английский) → строка`. Ключа нет в словаре —
   * виджет остаётся при своём английском, а не печатает ключ.
   * @param {string} key @param {string} en @param {Record<string, string|number>} [vars]
   */
  function widgetT(key, en, vars) {
    if (has(key)) return t(key, vars);
    let s = en;
    if (vars) for (const k of Object.keys(vars)) s = s.split(`{${k}}`).join(String(vars[k]));
    return s;
  }

  /** @param {ParentNode} [root] */
  function apply(root) {
    const r = root || document;
    r.querySelectorAll('[data-i18n]').forEach((el) => { el.textContent = t(/** @type {HTMLElement} */ (el).dataset.i18n); });
    r.querySelectorAll('[data-i18n-title]').forEach((el) => { /** @type {HTMLElement} */ (el).title = t(/** @type {HTMLElement} */ (el).dataset.i18nTitle); });
    r.querySelectorAll('[data-i18n-tip]').forEach((el) => { /** @type {HTMLElement} */ (el).dataset.tip = t(/** @type {HTMLElement} */ (el).dataset.i18nTip); });
    r.querySelectorAll('[data-i18n-ph]').forEach((el) => { /** @type {HTMLInputElement} */ (el).placeholder = t(/** @type {HTMLElement} */ (el).dataset.i18nPh); });
    if (typeof document !== 'undefined' && document.documentElement) document.documentElement.lang = lang;
  }

  /** @param {string} next */
  function setLang(next) {
    const l = norm(next);
    if (!l || l === lang) return;
    lang = l;
    try { localStorage.setItem(KEY, l); } catch { /* */ }
    apply();
    listeners.forEach((f) => { try { f(lang); } catch (e) { console.error('[i18n] слушатель смены языка упал', e); } });
  }

  /**
   * Текст отказа границы на языке интерфейса.
   * @param {number} status @param {string} [code]
   */
  function rejection(status, code) {
    if (lang === 'ru') {
      if (code && Object.prototype.hasOwnProperty.call(REJECT_RU, code)) return REJECT_RU[code];
      if (status === 401) return 'Нет сессии кабинета — торговля недоступна';
      return code ? `Отказ (${status}): терминал не знает причину «${code}»` : `Отказ (${status})`;
    }
    return window.Rejection ? window.Rejection.text(status, code) : `Rejected (${status})`;
  }

  /** Локаль для чисел и дат. Цены — всегда с точкой и без пробелов-разделителей
   *  в полях ввода; здесь только подписи. */
  function locale() { return lang === 'ru' ? 'ru-RU' : 'en-US'; }

  window.I18n = {
    t, has, widgetT, apply, setLang, rejection, locale, detect,
    lang: () => lang,
    onChange: (/** @type {(l: string) => void} */ f) => { listeners.push(f); },
    LANGS, DICT: D, REJECT_RU,
  };
})();
