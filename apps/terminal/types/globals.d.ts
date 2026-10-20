// Глобалы терминала для проверки типов (TRM-21).
//
// Модули по ADR-015 — IIFE, которые кладут себя в `window.<Имя>`. Сборки нет и
// не будет, поэтому импортов тоже нет, и единственный способ объяснить
// проверяющему, что эти имена существуют, — объявить их здесь.
//
// Файл НЕ отдаётся браузеру: он не попадает в образ (собирается `public/`) и
// ничего не исполняет. Это описание того, что уже есть, а не новый контракт.

interface TerminalConfig {
  brandUrl?: string;
  site?: string;
  locale?: string;
  cabinetOrigins?: string[];
}

interface Window {
  TERMINAL_CONFIG?: TerminalConfig;
  // Есть у настоящего окна, но в lib.dom объявлены глобальными, а не на
  // Window. Тестам они нужны через `env.window`, потому что окно там своё.
  console: Console;
  Event: typeof Event;
  MouseEvent: typeof MouseEvent;
  Api?: any;
  Theming?: any;
  Sso?: any;
  Diag?: any;
  Indicators?: any;
  OrderBook?: any;
  AssetStats?: any;
  Watchlist?: any;
  Positions?: any;
  Trades?: any;
  Drawings?: any;
  LightweightCharts?: any;
  /** Журнал родителя в браузерных проверках SSO. */
  __got?: any[];
  /** Отправка сообщения в iframe из страницы-родителя. */
  __send?: (m: any) => void;
}

// Те же модули видны и без префикса `window.`: страница грузит их тегами
// <script>, и внутри `app.js` они просто имена в глобальной области.
declare const Api: any;
declare const Theming: any;
declare const Sso: any;
declare const Diag: any;
declare const Indicators: any;
declare const OrderBook: any;
declare const AssetStats: any;
declare const Watchlist: any;
declare const Positions: any;
declare const Trades: any;
declare const Drawings: any;
/** Вендорная библиотека графика: вшивается в образ, типов не поставляет. */
declare const LightweightCharts: any;
