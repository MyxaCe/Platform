---
tags: [service, web]
status: active
updated: 2026-07-19
---

# Web UI (терминал)

## Назначение

Браузерный торговый терминал (стиль профессиональной платформы, ADR-012): верхняя панель метрик,
левый список инструментов, свечной график с таймфреймами, правая панель BUY/SELL, нижняя лента
сделок. На реальных данных [[gateway]]. Статический SPA (vanilla JS + TradingView Lightweight
Charts) за nginx. Каталог `web/`.

## Запуск

```bash
docker compose up --build   # gateway (внутренний) + web (nginx)
# → http://localhost:8888
```

## Компоненты и данные

- **Верх**: метрики BALANCE/EQUITY/FREE (из `/balance` котируемого актива текущего пользователя),
  переключатель Alice/Bob, статус WS.
- **Слева**: список пар — опрос `GET /instruments` каждую 1с (символ, change%, sell=bid, buy=ask, spread).
  Клик по строке → выбор инструмента. Поиск фильтрует список.
- **Центр**: тулбар таймфреймов (1M…1D) → `GET /candles/{id}?tf=`; свечи + объём; OHLC по курсору;
  водяной знак с символом. Live-обновление последней свечи из WS-сделок + периодический refetch (4с).
- **Справа**: вкладки OPEN DEAL (рынок) / LIMIT ORDER; лот-степпер; BUY/SELL с текущими ask/bid →
  `POST /orders` с Bearer-токеном.
- **Низ**: лента MARKET TRADES по всем парам из WS `/stream`.

Масштаб цен/объёмов — по `price_decimals`/`qty_decimals` из `/instruments` (хардкода нет).

## Связи

- Общается только с [[gateway]] по HTTP/WS через nginx-прокси (`web/nginx.conf`, один origin — без CORS).
- Основано на [[ADR-011-web-ui]], [[ADR-012-market-data-and-terminal-ui]].

## Возможности

- Данные **реальные с Binance** (ADR-013); **сделки бумажные** ([[broker]], ADR-014).
- **Терминальные фичи:** таймфреймы 1M…**1W**; **типы графика** (Candles/Hollow/Bars/Line/Area/**Heikin
  Ashi**) — кнопка-выпадашка; price type (Mid/Ask/Bid); **PIPS**-кнопка; SL/TP-линии на графике; лимитные
  ордера; вкладки OPEN DEALS / LIMIT ORDERS / CLOSED DEALS / SIGNALS.
- **Индикаторы** (кнопка Indicators, по категориям; считаются на клиенте из свечей):
  - *Оверлеи:* SMA, EMA, WMA, SuperTrend, Parabolic SAR, Ichimoku, Alligator, Bollinger, Keltner,
    Donchian, VWAP.
  - *Осциллятор (один, подвал):* RSI, Stochastic, CCI, Momentum, Williams %R, ROC, ADX/+DI/−DI, MACD,
    ATR, StdDev, OBV, A/D, CMF, MFI, Force Index, Awesome, Accelerator, Gator.
- **SIGNALS** — наши простые сигналы (RSI, MA-крест), **не Autochartist** (проприетарный, недоступен) — в отладке.
- UI-полировка: кастомные скроллбары, кнопки-выпадашки вместо `select`.
- **Стакан (order book)** в правой панели — реальная глубина Binance (`/depth`, опрос ~700мс).
  **Клик по уровню** подставляет цену в лимитную заявку (переключает на LIMIT).
- **Инструменты рисования** (`web/public/drawings.js` — отдельный модуль, оверлей-canvas над
  графиком, свой вертикальный тулбар): trend line, ray, extended line, info line, trend angle,
  horizontal line/ray, vertical line, cross line, **parallel channel**, **pitchfork (вилы Эндрюса)**,
  undo, clear. Якоря в координатах данных (logical-индекс + цена) → двигаются/масштабируются с
  графиком; очищаются при смене инструмента/ТФ (событие `chartReload`, logical-индексы становятся
  невалидными). Esc/ПКМ — отмена. Хук в app.js: `window.__lw = {chart, el, series()}`.
- **Размер сделки в LOT или USD** — переключатель; при USD объём считается как `usd / цена`, показывается
  эквивалент (≈ монет / ≈ $). Для клиентов, которым непонятен LOT.
- **Статистика актива под графиком** — изменение (%) по 1m/5m/15m/30m/1h/4h/1d/1w (`/stats`, опрос ~5с).
- **Иконки** — встроенный набор SVG (`ICONS` в app.js, без внешнего CDN): тип графика (иконка меняется
  под выбор: Candles/Hollow/Bars/Line/Area), Indicators (двойная кривая), PIPS (кошелёк), звёздочки-избранное.
- **Тулбар:** тип графика (иконка+название), Indicators (иконка), price type (текст MID/ASK/BID),
  **SL/TP** (текст-тумблер: показать/скрыть линии, акцент когда вкл), **PIPS** (кошелёк-тумблер).
- **Инфо-окно на графике** (legend): символ, ТФ, дата/время, O/H/L/C (обновляется по курсору и вживую).
- **Список монет:** звёздочка-**избранное** (в localStorage, избранные наверх) + колонки CHANGE/SELL/BUY/SPREAD/**HIGH**.
- **Метрики и тулбар** растянуты на всю ширину (`space-between`; правая группа тулбара — `margin-left:auto`).
  Тулбар **переносит группы кнопок и растёт вниз** (`flex-wrap` + `min-height`): на экране 1280px он
  не помещается в одну строку — одни таймфреймы занимают 289px. Ресайзер задаёт минимальную высоту,
  а не жёсткую (BUG-010).
- **Ресайз панелей** перетаскиванием (левая/правая/нижняя; размеры в localStorage).
- **Кнопки тулбара справа:** Reset (сброс зума на ~90 баров), Fullscreen (Fullscreen API на графике), Settings (⚙).
- **Настройки** (⚙, в localStorage): Hollow candle color; Candle shadows вкл/выкл + свой цвет теней;
  Price line width (0 = выкл); Additional price line вкл/выкл + width; Minimal price change (Default / 1:1 / 1:10).

## Модульная архитектура (ADR-015)

Модули переиспользуемы: самодостаточны, зависимости инъектируются через контекст, глобалы и id
страницы под запретом. Контракт виджета — `Widget.mount(rootEl, ctx) → { refresh, destroy }`.

- `api.js` — `Api.create({ token })`: единственная точка доступа к REST. Заголовок авторизации и
  разбор ответов собраны здесь, а не продублированы по виджетам. Соглашение: чтение → данные или
  `null` при ошибке (вызывающий оставляет прежние данные на экране); мутация → `{ ok, status, data }`.
  Ноль DOM. Готовит переход на настоящий auth — правка будет в одном месте.
- `indicators.js` — `window.Indicators`, ~35 чистых функций (мат. индикаторов). Ноль DOM/глобалов.
- `orderbook.js` — `OrderBook.mount(el, ctx)`: сам строит стакан, `ctx.instrument()/fetch(id)/onPick`.
- `stats.js` — `AssetStats.mount(el, ctx)`: статистика по периодам, `ctx.instrument()/fetch(id)/labels`.
- `watchlist.js` — `Watchlist.mount(el, ctx)`: список инструментов целиком — поиск, шапка с
  сортировкой, строки, избранное, видимость колонок. Строит свой DOM (страница даёт пустой
  `<aside class="watch">`), цены форматирует сам по `price_decimals` из данных. Наружу:
  `refresh() / setActive(id) / columns() / setColumns(next) / destroy()`; внутрь через ctx —
  `fetch() / instrument() / onSelect(id) / onData(list) / interval / storagePrefix`.
- `drawings.js` — инструменты рисования (оверлей-canvas), через `window.__lw`. **Контракту ADR-015
  пока не соответствует** (тянет глобал, хардкодит id) — в очереди на перенос.
- `app.js` — тонкий композитор: связывает виджеты через `ctx`, держит общее состояние.

Подключение по порядку: lightweight-charts → api → indicators → orderbook → stats → watchlist →
app → drawings.
**Все новые модули — по этому паттерну; старые переносятся постепенно.** См. [[ADR-015-reusable-frontend-modules]].

Осталось вынести: панель сделки, нижние вкладки (deals/pending/closed/signals), клиент-фид (WS +
интервалы), лейаут/настройки, график, `drawings.js`.

## Ограничения / TODO

- Индикаторы клиентские и базовые; **Ichimoku без сдвига облака, Alligator без сдвига** (упрощено).
- Скоро: **Volume Profile, TPO, Delta, Fibonacci, ZigZag, Pivot Points** (нужен особый рендеринг/данные). [[backlog]]
- Плечо/ликвидация, комиссии — [[backlog]].
- Только крипто; форекс/сырьё/акции/индексы — платный провайдер, [[backlog]].
- Токены Alice/Bob в UI (demo) — до настоящего auth. [[backlog]]
