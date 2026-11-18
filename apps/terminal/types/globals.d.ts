interface TerminalConfig {
  brandUrl?: string;
  site?: string;
  locale?: string;
  cabinetOrigins?: string[];
}

interface Window {
  TERMINAL_CONFIG?: TerminalConfig;
  console: Console;
  Event: typeof Event;
  MouseEvent: typeof MouseEvent;
  Api?: any;
  Theming?: any;
  Sso?: any;
  I18n?: any;
  Diag?: any;
  Rejection?: any;
  Tariff?: any;
  Stops?: any;
  Levels?: any;
  Indicators?: any;
  OrderBook?: any;
  AssetStats?: any;
  Watchlist?: any;
  Positions?: any;
  Trades?: any;
  Drawings?: any;
  Series?: any;
  LightweightCharts?: any;
  __got?: any[];
  __send?: (m: any) => void;
}

declare const Api: any;
declare const Theming: any;
declare const Sso: any;
declare const I18n: any;
declare const Diag: any;
declare const Rejection: any;
declare const Tariff: any;
declare const Stops: any;
declare const Levels: any;
declare const Indicators: any;
declare const OrderBook: any;
declare const AssetStats: any;
declare const Watchlist: any;
declare const Positions: any;
declare const Trades: any;
declare const Drawings: any;
declare const Series: any;
declare const LightweightCharts: any;
