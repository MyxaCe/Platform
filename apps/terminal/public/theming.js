'use strict';
// ============================================================================
// theming.js — адаптер темизации терминала под theming-пакет платформы (v1).
// Фаза Т1 интеграции (ADR-023). Контракт — theming-pack/README.md.
//
// Что делает:
//   1) выставляет тему через атрибут data-theme (dark по умолчанию, light);
//   2) грузит бренд сайта из CMS (GET /v1/cms/brand), валидирует по контракту,
//      применяет accent-цвет, логотип и имя; кеширует last-good;
//   3) автономность: без CMS терминал остаётся на палитре платформы (tokens.v1.css) —
//      «никогда не раскрашиваться мусором».
//
// Цвета платформы приходят RGB-каналами (tokens.v1.css). Бренд переопределяет
// канал --accent на :root — этого достаточно, чтобы перекрасился весь UI и график
// (мост в style.css: --brand: rgb(var(--accent)), график читает канал напрямую).
// ============================================================================
(function () {
  const THEME_KEY = 'terminal:theme';
  const BRAND_KEY = 'terminal:brand.lastgood';
  const REFRESH_MS = 5 * 60 * 1000;

  // ---- Тема ----------------------------------------------------------------
  const root = document.documentElement;
  function normTheme(t) {
    return t === 'light' || t === 'dark' ? t : null;
  }
  function currentTheme() {
    return root.getAttribute('data-theme') || 'dark';
  }
  function setTheme(t) {
    const v = normTheme(t) || 'dark';
    root.setAttribute('data-theme', v);
    try { localStorage.setItem(THEME_KEY, v); } catch { /* приватный режим */ }
    document.dispatchEvent(new Event('themechange'));
  }
  function toggleTheme() {
    setTheme(currentTheme() === 'dark' ? 'light' : 'dark');
  }
  // Первичная тема — синхронно, ДО построения графика в app.js: приоритет
  // ?theme= (сайт передаёт выбор пользователя) → сохранённая → dark.
  (function initTheme() {
    const q = new URLSearchParams(location.search).get('theme');
    let saved = null;
    try { saved = localStorage.getItem(THEME_KEY); } catch { /* */ }
    root.setAttribute('data-theme', normTheme(q) || normTheme(saved) || 'dark');
  })();

  // ---- Цвет: hex → каналы, яркость, осветление -----------------------------
  /**
   * @param {string} hex
   * @returns {[number, number, number] | null} ровно три канала или `null`
   */
  function hexToChannels(hex) {
    const m = /^#([0-9a-fA-F]{6})$/.exec(hex);
    if (!m) return null;
    const n = parseInt(m[1], 16);
    return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
  }
  // Относительная яркость (sRGB) — чтобы выбрать читаемый текст поверх акцента.
  function luminance([r, g, b]) {
    const f = (c) => { c /= 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; };
    return 0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b);
  }
  function lighten([r, g, b], k) {
    const up = (c) => Math.round(c + (255 - c) * k);
    return [up(r), up(g), up(b)];
  }

  // ---- Чтение ответа бренда: две формы, один канонический вид --------------
  //
  // CMS v2 отдаёт бренд публичным ресурсом БЕЗ ключа (Р-039, ADR-0035):
  //   GET {CMS}/v1/public/sites/{slug}/brand
  //   { contract, site: { slug, name },
  //     brand: { logoLight, logoDark, logoMono, logoMark, favicon, emailLogo,
  //              primaryColor, socials } }
  // Поле, которое мы читали как `logo`, там называется `logoLight`, имя бренда
  // переехало из корня в `site.name`, а слаг — из параметра в путь (TRM-12).
  //
  // Legacy-ответ `GET {CMS}/v1/cms/brand?site=&locale=` плоский:
  //   { name, primaryColor, logo, favicon }
  //
  // Читаем ОБЕ формы, и это не перестраховка. Переезд контура на v2 и выкатка
  // терминала — разные моменты, и порядок их нам не подчиняется: потребитель,
  // умеющий только новую форму, остаётся без логотипа ровно в момент переезда,
  // умеющий только старую — сразу после него. Обе формы снимают вопрос порядка.
  //
  // Жёсткость здесь опаснее мягкости. Прежняя проверка требовала валидный
  // `primaryColor` и отвергала ВЕСЬ ответ, если его нет, — а в v2 это поле
  // объявлено обнуляемым. Бренд с логотипом и именем, но без цвета, откатил бы
  // терминал на кеш или на палитру платформы и показал бы чужое имя при живой
  // CMS. Тот же дефект уже стоил кабинету бренда целиком: проверка на одно поле,
  // отказ за весь объект. Поэтому поля читаются по отдельности, и негодное
  // поле стоит своего поля, а не всего бренда.

  /** Картинка бренда. `width/height/mimeType` для отрисовки не нужны — `<img>`
   *  возьмёт размеры сам, — и требовать их значило бы отвергнуть годную
   *  картинку из-за поля, которым мы не пользуемся. */
  function readImage(v) {
    if (!v || typeof v !== 'object') return null;
    if (typeof v.url !== 'string' || v.url.length === 0) return null;
    return { url: v.url, alt: typeof v.alt === 'string' ? v.alt : '' };
  }

  function readColor(v) {
    return typeof v === 'string' && /^#[0-9a-fA-F]{6}$/.test(v) ? v : null;
  }

  function readName(v) {
    return typeof v === 'string' && v.trim() !== '' ? v : null;
  }

  /**
   * Приводит ответ любой из двух форм к одному виду.
   * `null` — форма не опознана вовсе: раскрашиваться мусором мы не станем.
   */
  function normalizeBrand(raw) {
    if (!raw || typeof raw !== 'object') return null;

    // Признак v2 — вложенные объекты `site` и `brand`. Опознаём по форме, а не
    // по полю `contract`: оно там равно "v1" (версия контракта выдачи, не
    // версия CMS), и ориентироваться на него значит поставить ловушку
    // следующему читателю.
    const v2 = !!raw.brand && typeof raw.brand === 'object'
      && !!raw.site && typeof raw.site === 'object';
    const src = v2 ? raw.brand : raw;
    const name = readName(v2 ? raw.site.name : raw.name);

    // Плоский ответ без имени неотличим от чужого JSON — например от страницы
    // ошибки прокси. Для v2 опознавательный признак даёт сама форма.
    if (!v2 && name === null) return null;

    const brand = {
      shape: v2 ? 'v2' : 'legacy',
      name: name,
      primaryColor: readColor(src.primaryColor),
      logoLight: readImage(v2 ? src.logoLight : src.logo),
      logoDark: v2 ? readImage(src.logoDark) : null,
      favicon: readImage(src.favicon),
    };

    // Ответ опознан, но пригодного в нём нет ничего. Применять нечего, и
    // «применили» было бы неправдой: отличать это от успеха обязан вызывающий,
    // иначе в last-good ляжет пустышка.
    if (!brand.name && !brand.primaryColor && !brand.logoLight && !brand.favicon) {
      return null;
    }

    return brand;
  }

  /**
   * Подставлять ли светлый логотип на тёмной теме (Р-063).
   *
   * Решение штаба: **нет**, и оно перебивает прежнее прочтение этой команды.
   * Рассуждение: `logoLight` по названию предназначен для светлого фона, то
   * есть вероятно нарисован тёмными чернилами. На тёмной теме он не «хуже
   * тёмного варианта», а **невидим** — вышло бы пустое место, про которое код
   * считает, что логотип показан. Это наш класс «отказ, надевший правдоподобный
   * вид»: пустой знак неотличим от знака, который не загрузился.
   *
   * Пока тёмного варианта нет — знак уступает место имени сайта: инициал в
   * самом знаке и полное имя рядом, в `#brandName`. Текст виден на любом фоне и
   * честно сообщает, что картинки нет.
   *
   * Вопрос о семантике `logoLight` ушёл в `cms`. Если она имела в виду
   * «основной знак, пригодный на любом фоне», решение меняется обратно —
   * **переключением одной константы ниже**, как и просил штаб. Больше нигде
   * выбор логотипа не зашит.
   */
  const LIGHT_LOGO_WORKS_ON_DARK = false;

  /**
   * Решение ограничено формой v2, и это не лазейка.
   *
   * Довод Р-063 — про **имя поля**: `logoLight` назван по фону, для которого
   * предназначен. У legacy поле называется `logo`, фон в его имени не назван, и
   * терминал показывает его на своей тёмной теме с самого начала — то есть
   * известно, что оно там видно. Распространить запрет на legacy значило бы
   * сломать работающий экран доводом, который к его полю не относится.
   *
   * Когда контур переедет на v2, legacy-ветка уйдёт вместе с ним, и останется
   * одна константа выше.
   */
  /**
   * **Условие, на котором это держится (Р-066), а не разрешение.**
   *
   * Эмпирика здесь — один наблюдавшийся набор ассетов на текущем контуре, а не
   * свойство поля `logo`. Отступление безопасно ровно потому, что **ветка
   * умирает при переезде на v2**: тогда останется один `logoLight` и одна
   * константа выше.
   *
   * Если переезд задержится и в legacy заведут нового тенанта со светлым
   * знаком, отступление перестанет быть безопасным — и перестанет **молча**:
   * у тенанта просто пропадёт логотип на тёмной теме, без ошибки. Это наш
   * класс мины: допущение, которое планируется отменить, и связь с будущим
   * событием, нигде не записанная.
   *
   * Поэтому записана здесь: **кто ведёт переезд на v2 — обезвреживает это в
   * том же заходе**, а не «когда дойдут руки». Пока v2 не пришёл, новый
   * legacy-тенант со светлым знаком — повод вернуться сюда, а не к бренду.
   */
  function lightLogoUsableOnDark(brand) {
    return brand.shape === 'legacy' ? true : LIGHT_LOGO_WORKS_ON_DARK;
  }

  /** Какой знак показать в текущей теме. `null` — знака нет, нужна подпись. */
  function pickLogo(brand) {
    if (currentTheme() !== 'dark') return brand.logoLight;
    if (brand.logoDark) return brand.logoDark;
    return lightLogoUsableOnDark(brand) ? brand.logoLight : null;
  }

  // ---- Применение бренда ---------------------------------------------------

  /** Последний применённый бренд: логотип зависит от темы, а тему меняют и
   *  после загрузки бренда. */
  let applied = null;

  function renderMark(brand) {
    const mark = document.querySelector('.logo__mark');
    if (!mark) return;

    const logo = pickLogo(brand);
    if (logo) {
      const cur = mark.querySelector('img');
      // Переиспользуем, а не пересоздаём: иначе каждая смена темы заставляла бы
      // браузер грузить ту же картинку заново.
      if (cur && cur.getAttribute('src') === logo.url) return;
      mark.innerHTML = '';
      const img = document.createElement('img');
      img.src = logo.url;
      img.alt = logo.alt || brand.name || '';
      mark.appendChild(img);
      return;
    }

    // Логотипа нет — буква имени. Если нет и имени, НЕ трогаем знак вовсе:
    // поставить сюда заглушку значило бы стереть то, что уже нарисовано, ради
    // поля, о котором CMS ничего не сказала.
    if (brand.name) mark.textContent = brand.name.trim().charAt(0).toUpperCase();
  }

  function applyBrand(raw) {
    const brand = normalizeBrand(raw);
    if (!brand) return false;

    if (brand.primaryColor) {
      const ch = hexToChannels(brand.primaryColor);
      if (ch) {
        root.style.setProperty('--accent', ch.join(' '));
        root.style.setProperty('--accent-hover', lighten(ch, 0.12).join(' '));
        // Тёмный акцент → светлый текст на нём, и наоборот (порог по яркости).
        root.style.setProperty('--on-accent', luminance(ch) > 0.4 ? '#08090c' : '#f5f6f8');
      }
    }

    if (brand.name) {
      const brandEl = document.getElementById('brandName');
      if (brandEl) brandEl.textContent = brand.name;
      document.title = brand.name + ' · Terminal';
    }

    applied = brand;
    renderMark(brand);

    // Фавикон живёт на вкладке браузера, а не в нашей теме, и выбора темы не
    // знает. Поэтому запасной для него — `logoLight` напрямую, а не знак,
    // выбранный `pickLogo`: иначе на тёмной теме вкладка осталась бы без
    // иконки по причине, к вкладке отношения не имеющей.
    const fav = brand.favicon || brand.logoLight;
    if (fav) {
      let link = /** @type {HTMLLinkElement} */ (document.querySelector('link[rel="icon"]'));
      if (!link) { link = document.createElement('link'); link.rel = 'icon'; document.head.appendChild(link); }
      link.href = fav.url;
    }

    document.dispatchEvent(new Event('themechange')); // график перечитает акцент
    return true;
  }

  // Смена темы меняет и логотип. Слушатель только перерисовывает знак и сам
  // события не шлёт — иначе `applyBrand` и он закольцевались бы.
  document.addEventListener('themechange', function () {
    if (applied) renderMark(applied);
  });

  // ---- Загрузка из CMS -----------------------------------------------------
  function cacheBrand(b) {
    try { localStorage.setItem(BRAND_KEY, JSON.stringify(b)); } catch { /* */ }
  }
  function loadCached() {
    try {
      const raw = localStorage.getItem(BRAND_KEY);
      if (raw) applyBrand(JSON.parse(raw));
    } catch { /* нет валидного кеша — остаёмся на палитре платформы */ }
  }
  /**
   * Куда идти за брендом.
   *
   * В v2 слаг сайта стоит В ПУТИ: `{CMS}/v1/public/sites/{slug}/brand`. В
   * конфиге такой адрес задаётся с плейсхолдером `{site}`; адрес без
   * плейсхолдера считается legacy и получает слаг параметром.
   *
   * Ключа нет ни там, ни там, и это не упущение: публичный ресурс v2 выдаётся
   * без ключа намеренно (Р-039) — тему терминала браузер грузит до логина.
   *
   * `null` — спрашивать нельзя. Адрес со слагом в пути без слага собрался бы в
   * `/sites//brand`: это был бы запрос чужого или никакого бренда, то есть
   * правдоподобный неверный ответ вместо отказа.
   */
  function brandRequestUrl(cfg) {
    const base = String((cfg && cfg.brandUrl) || '').trim();
    if (!base) return null; // адреса нет — остаёмся на палитре платформы

    const site = (cfg && cfg.site)
      || (typeof location !== 'undefined' ? new URLSearchParams(location.search).get('site') : '')
      || '';

    if (base.indexOf('{site}') !== -1) {
      if (!site) return null;
      return base.replace(/\{site\}/g, encodeURIComponent(site));
    }

    // Legacy: тенант параметром (ADR-023), локаль туда же. У публичного ресурса
    // v2 локали нет вовсе — бренд от неё не зависит.
    const qs = new URLSearchParams();
    if (site) qs.set('site', site);
    qs.set('locale', (cfg && cfg.locale) || 'en');
    return `${base.replace(/\/$/, '')}?${qs}`;
  }

  async function fetchBrand(cfg) {
    const url = brandRequestUrl(cfg);
    if (!url) return;
    try {
      const r = await fetch(url, { credentials: 'omit' });
      if (!r.ok) return;
      const brand = await r.json();
      if (applyBrand(brand)) cacheBrand(brand); // кешируем только валидный (last-good)
    } catch { /* прокси/сеть недоступны — держим текущий бренд */ }
  }

  // ---- Инициализация -------------------------------------------------------
  let timer = null;
  function init(overrides) {
    // brandUrl — полный путь к brand-прокси платформы (напр. http://localhost:3002/v1/cms/brand).
    // Ключа CMS здесь нет и быть не должно — он живёт на стороне прокси (server-side).
    const cfg = Object.assign({ brandUrl: '', site: '', locale: 'en' }, window.TERMINAL_CONFIG || {}, overrides || {});
    loadCached();              // мгновенно показать прошлый валидный бренд
    fetchBrand(cfg);           // и обновить из CMS через прокси платформы
    if (cfg.brandUrl) {
      if (timer) clearInterval(timer);
      timer = setInterval(() => fetchBrand(cfg), REFRESH_MS);
    }
    return { setTheme, toggleTheme, applyBrand, refresh: () => fetchBrand(cfg) };
  }

  // `normalizeBrand` и `brandRequestUrl` экспортируются ради тестов, и это
  // названо вслух: обе — чистые функции, в которых живёт вся разница между
  // формой ответа v2 и legacy. Проверять их через сеть значило бы проверять
  // заодно `fetch`, а читать глазами — не проверять вовсе.
  window.Theming = {
    init, applyBrand, setTheme, toggleTheme, currentTheme, normalizeBrand, brandRequestUrl,
  };
})();
