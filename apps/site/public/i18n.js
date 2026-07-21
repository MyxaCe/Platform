/* ============================================================================
 * i18n.js — переключение языка сайта. Тексты в разметке помечены data-i18n;
 * словарь один на страницу, добавление языка = добавление объекта сюда.
 *
 *   I18n.setLang('ru');  I18n.t('nav.markets');  I18n.lang();
 *
 * По умолчанию английский — как и в терминале; русский доступен переключателем.
 * ========================================================================== */
(function () {
  const KEY = 'site.lang';

  const DICT = {
    en: {
      'nav.buy': 'Buy Crypto', 'nav.markets': 'Markets', 'nav.trade': 'Trade', 'nav.futures': 'Futures',
      'nav.earn': 'Earn', 'nav.square': 'Square', 'nav.more': 'More',
      'cta.login': 'Log In', 'cta.register': 'Sign Up', 'cta.start': 'Get Started', 'cta.terminal': 'Open Terminal',
      'search.ph': 'Search markets',
      'hero.title': 'Trade crypto on real market data',
      'hero.lead': 'A professional terminal with a live order book, charts and risk tools — built on live exchange feeds.',
      'hero.note': 'Demo environment. Paper trading on live prices — no real funds are involved.',
      'markets.title': 'Markets', 'markets.sub': 'Live prices from the exchange feed.',
      'markets.pair': 'Pair', 'markets.price': 'Price', 'markets.change': '24h Change', 'markets.high': '24h High',
      'markets.empty': 'Nothing found', 'markets.loading': 'Loading markets…',
      'feat.title': 'Built for serious trading',
      'feat.charts.t': 'Charts and indicators',
      'feat.charts.d': 'Candles, Heikin Ashi, bars and lines with ~35 built-in indicators and drawing tools.',
      'feat.book.t': 'Live order book',
      'feat.book.d': 'Real depth with price grouping, cumulative view and buy/sell pressure.',
      'feat.size.t': 'Order size in lots or USD',
      'feat.size.d': 'Enter an amount the way you think about it — the equivalent is calculated for you.',
      'feat.risk.t': 'Stop loss and take profit',
      'feat.risk.d': 'Attach protective levels to any position and see them right on the chart.',
      'steps.title': 'Start in three steps',
      'steps.1.t': 'Create an account', 'steps.1.d': 'Sign up with your email — it takes a minute.',
      'steps.2.t': 'Fund your balance', 'steps.2.d': 'Top up and pick the instrument you want to trade.',
      'steps.3.t': 'Open your first position', 'steps.3.d': 'Trade from the terminal with live prices and protective levels.',
      'cta.block.t': 'Ready to trade?',
      'cta.block.d': 'Open the terminal and see the market exactly as it is right now.',
      'ftr.note': 'Demo environment built on live exchange data. Paper trading only — no real funds.',
      'ftr.product': 'Product', 'ftr.company': 'Company', 'ftr.legal': 'Legal',
      'ftr.about': 'About', 'ftr.contact': 'Contact', 'ftr.careers': 'Careers',
      'ftr.terms': 'Terms', 'ftr.privacy': 'Privacy', 'ftr.risk': 'Risk disclosure',
      'auth.login.t': 'Log in', 'auth.login.s': 'Welcome back. Enter your details to continue.',
      'auth.register.t': 'Create an account', 'auth.register.s': 'A couple of details and you can start trading.',
      'auth.email': 'Email', 'auth.password': 'Password', 'auth.confirm': 'Confirm password',
      'auth.remember': 'Remember me', 'auth.forgot': 'Forgot password?',
      'auth.do.login': 'Log in', 'auth.do.register': 'Create account',
      'auth.noAccount': 'No account yet?', 'auth.haveAccount': 'Already have an account?',
      'auth.toRegister': 'Sign up', 'auth.toLogin': 'Log in',
      'auth.agree': 'I have read and agree to the {terms} and the {risk}',
      'auth.terms': 'Terms', 'auth.risk': 'Risk Disclosure',
      'auth.show': 'Show password', 'auth.hide': 'Hide password', 'auth.close': 'Close',
      'auth.strength': 'Password strength', 'auth.weak': 'weak', 'auth.medium': 'medium', 'auth.strong': 'strong',
      'auth.err.email': 'Enter a valid email address',
      'auth.err.pass': 'At least 8 characters, with a letter and a digit',
      'auth.err.confirm': 'Passwords do not match',
      'auth.err.agree': 'Please accept the terms to continue',
      'auth.soon': 'Sign-up is not open yet: this is a demo environment without a real account system.',
      'ftr.rights': '© 2026 ExchangePro. All rights reserved.',
      'ftr.warn': 'Trading involves risk. Never trade with money you cannot afford to lose.',
    },
    ru: {
      'nav.buy': 'Купить криптовалюту', 'nav.markets': 'Рынки', 'nav.trade': 'Торговля', 'nav.futures': 'Фьючерсы',
      'nav.earn': 'Earn', 'nav.square': 'Square', 'nav.more': 'Подробнее',
      'cta.login': 'Вход', 'cta.register': 'Регистрация', 'cta.start': 'Начать', 'cta.terminal': 'Открыть терминал',
      'search.ph': 'Поиск по рынкам',
      'hero.title': 'Торгуйте криптовалютой на реальных данных рынка',
      'hero.lead': 'Профессиональный терминал: живой биржевой стакан, графики и инструменты контроля риска на реальных котировках.',
      'hero.note': 'Демо-среда. Торговля бумажными деньгами по реальным ценам — реальные средства не задействованы.',
      'markets.title': 'Рынки', 'markets.sub': 'Живые котировки из биржевого фида.',
      'markets.pair': 'Пара', 'markets.price': 'Цена', 'markets.change': 'Изм. 24ч', 'markets.high': 'Максимум 24ч',
      'markets.empty': 'Ничего не найдено', 'markets.loading': 'Загружаем рынки…',
      'feat.title': 'Сделано для серьёзной торговли',
      'feat.charts.t': 'Графики и индикаторы',
      'feat.charts.d': 'Свечи, Heikin Ashi, бары и линии, около 35 встроенных индикаторов и инструменты рисования.',
      'feat.book.t': 'Живой биржевой стакан',
      'feat.book.d': 'Реальная глубина: группировка цен, накопительный режим и соотношение покупок и продаж.',
      'feat.size.t': 'Объём в лотах или в долларах',
      'feat.size.d': 'Вводите сумму так, как вам привычнее, — эквивалент посчитается сам.',
      'feat.risk.t': 'Стоп-лосс и тейк-профит',
      'feat.risk.d': 'Защитные уровни для любой позиции — видны прямо на графике.',
      'steps.title': 'Начните за три шага',
      'steps.1.t': 'Создайте аккаунт', 'steps.1.d': 'Регистрация по электронной почте занимает минуту.',
      'steps.2.t': 'Пополните баланс', 'steps.2.d': 'Внесите средства и выберите инструмент для торговли.',
      'steps.3.t': 'Откройте первую сделку', 'steps.3.d': 'Торгуйте из терминала по реальным ценам и с защитными уровнями.',
      'cta.block.t': 'Готовы торговать?',
      'cta.block.d': 'Откройте терминал и посмотрите на рынок таким, какой он прямо сейчас.',
      'ftr.note': 'Демо-среда на реальных биржевых данных. Только бумажная торговля — без реальных средств.',
      'ftr.product': 'Продукт', 'ftr.company': 'Компания', 'ftr.legal': 'Правовая информация',
      'ftr.about': 'О нас', 'ftr.contact': 'Контакты', 'ftr.careers': 'Вакансии',
      'ftr.terms': 'Условия', 'ftr.privacy': 'Конфиденциальность', 'ftr.risk': 'Раскрытие рисков',
      'auth.login.t': 'Вход', 'auth.login.s': 'С возвращением. Введите данные, чтобы продолжить.',
      'auth.register.t': 'Регистрация', 'auth.register.s': 'Пара данных — и можно начинать торговать.',
      'auth.email': 'Электронная почта', 'auth.password': 'Пароль', 'auth.confirm': 'Повторите пароль',
      'auth.remember': 'Запомнить меня', 'auth.forgot': 'Забыли пароль?',
      'auth.do.login': 'Войти', 'auth.do.register': 'Создать аккаунт',
      'auth.noAccount': 'Ещё нет аккаунта?', 'auth.haveAccount': 'Уже есть аккаунт?',
      'auth.toRegister': 'Зарегистрироваться', 'auth.toLogin': 'Войти',
      'auth.agree': 'Я прочитал и принимаю {terms} и {risk}',
      'auth.terms': 'Условия', 'auth.risk': 'Раскрытие рисков',
      'auth.show': 'Показать пароль', 'auth.hide': 'Скрыть пароль', 'auth.close': 'Закрыть',
      'auth.strength': 'Надёжность пароля', 'auth.weak': 'слабый', 'auth.medium': 'средний', 'auth.strong': 'надёжный',
      'auth.err.email': 'Введите корректный адрес электронной почты',
      'auth.err.pass': 'Минимум 8 символов, буква и цифра',
      'auth.err.confirm': 'Пароли не совпадают',
      'auth.err.agree': 'Примите условия, чтобы продолжить',
      'auth.soon': 'Регистрация пока закрыта: это демо-среда без настоящей системы аккаунтов.',
      'ftr.rights': '© 2026 ExchangePro. Все права защищены.',
      'ftr.warn': 'Торговля сопряжена с риском. Не торгуйте на средства, потерю которых не можете себе позволить.',
    },
  };

  let lang = localStorage.getItem(KEY) || 'en';
  if (!DICT[lang]) lang = 'en';

  const t = (key) => (DICT[lang] && DICT[lang][key]) || (DICT.en[key] ?? key);

  /** Проставить переводы в разметке. Вызывается после смены языка. */
  function apply(root = document) {
    root.querySelectorAll('[data-i18n]').forEach((el) => { el.textContent = t(el.dataset.i18n); });
    root.querySelectorAll('[data-i18n-ph]').forEach((el) => { el.placeholder = t(el.dataset.i18nPh); });
    document.documentElement.lang = lang;
  }

  const listeners = [];
  function setLang(next) {
    if (!DICT[next] || next === lang) return;
    lang = next;
    localStorage.setItem(KEY, lang);
    apply();
    listeners.forEach((fn) => fn(lang));
  }

  window.I18n = { t, apply, setLang, lang: () => lang, onChange: (fn) => listeners.push(fn) };
})();
