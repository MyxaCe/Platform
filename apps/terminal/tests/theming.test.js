'use strict';
// ============================================================================
// Бренд терминала: чтение ответа CMS и адрес, по которому за ним ходят (TRM-12).
//
// Поле, которое терминал читал как `logo`, в CMS v2 называется `logoLight`,
// имя бренда переехало из корня в `site.name`, а публичный ресурс сменил адрес
// на `/v1/public/sites/{slug}/brand`. Правка маленькая, но до переезда контура
// на v2 — иначе терминал остался бы без логотипа ровно в момент переезда.
//
// Тесты на настоящем DOM (jsdom, devDependency, Р-020): бренд — это запись в
// документ, и подделать её заглушкой значило бы проверить заглушку.
// ============================================================================
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadWidget } = require('./helpers/load-widget');

/** Поднять терминал с шапкой: знак бренда и название, как в index.html. */
function setup() {
  const env = loadWidget(['theming.js']);
  const { document } = env;
  document.body.innerHTML =
    '<div class="logo"><span class="logo__mark">E</span> <span id="brandName">Exchange</span></div>';
  return env;
}

const img = (url, alt) => ({ url, width: 100, height: 40, alt, mimeType: 'image/svg+xml' });

/** Ответ CMS v2: публичный ресурс бренда. */
function v2(over) {
  return Object.assign(
    {
      contract: 'v1',
      site: { slug: 'apex-ru', name: 'Apex' },
      brand: {
        logoLight: img('https://media/apex-light.svg', 'Apex'),
        logoDark: img('https://media/apex-dark.svg', 'Apex'),
        logoMono: null,
        logoMark: null,
        favicon: img('https://media/apex.ico', 'Apex'),
        emailLogo: null,
        primaryColor: '#ff6600',
        socials: [],
      },
    },
    over || {},
  );
}

/** Ответ legacy: плоская форма `GET /v1/cms/brand`. */
function legacy(over) {
  return Object.assign(
    {
      name: 'Старый бренд',
      primaryColor: '#0066ff',
      logo: img('https://media/legacy.svg', 'Старый бренд'),
      favicon: img('https://media/legacy.ico', 'Старый бренд'),
    },
    over || {},
  );
}

const markSrc = (document) => {
  const el = document.querySelector('.logo__mark img');
  return el ? el.getAttribute('src') : null;
};

// ---- Форма v2 --------------------------------------------------------------

test('v2: имя берётся из site.name, логотип из logoLight', (t) => {
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  assert.equal(window.Theming.applyBrand(v2()), true);

  assert.equal(document.getElementById('brandName').textContent, 'Apex');
  assert.equal(document.title, 'Apex · Terminal');
  // Тема по умолчанию тёмная, поэтому ожидаем тёмный вариант.
  assert.equal(markSrc(document), 'https://media/apex-dark.svg');
});

test('v2: акцент применяется каналами RGB', (t) => {
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  window.Theming.applyBrand(v2());

  assert.equal(document.documentElement.style.getPropertyValue('--accent').trim(), '255 102 0');
});

test('v2: поле logo не читается — его там нет', (t) => {
  // Регресс на саму суть TRM-12. Если бы код по-прежнему читал `logo`,
  // ответ v2 остался бы без логотипа при живой CMS и без единой ошибки.
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  const brand = v2();
  brand.brand.logoLight = null;
  brand.brand.logoDark = null;
  brand.logo = img('https://media/НЕ-ОТТУДА.svg', 'нет');

  window.Theming.applyBrand(brand);

  assert.equal(markSrc(document), null, 'картинки быть не должно');
  assert.equal(document.querySelector('.logo__mark').textContent, 'A', 'остаётся буква имени');
});

// ---- Форма legacy ----------------------------------------------------------

test('legacy: плоский логотип показывается и на тёмной теме', (t) => {
  // Запрет Р-063 намеренно НЕ распространён на legacy: довод там про имя поля
  // `logoLight`, а здесь поле называется `logo` — фон в его имени не назван, и
  // терминал показывает его на тёмной теме с самого начала. Ломать работающий
  // экран доводом, который к его полю не относится, мы не стали.
  // Обе формы обязаны работать: момент переезда контура на v2 нам не
  // подчиняется, и потребитель, умеющий только одну форму, остаётся без
  // логотипа либо до переезда, либо после.
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  assert.equal(window.Theming.applyBrand(legacy()), true);

  assert.equal(document.getElementById('brandName').textContent, 'Старый бренд');
  assert.equal(markSrc(document), 'https://media/legacy.svg');
  assert.equal(document.documentElement.style.getPropertyValue('--accent').trim(), '0 102 255');
});

// ---- Мягкость потребителя --------------------------------------------------

test('v2 без фирменного цвета: бренд применяется, а не отвергается целиком', (t) => {
  // Прежняя проверка требовала валидный primaryColor и отвергала ВЕСЬ ответ,
  // если его нет. В v2 поле обнуляемо по контракту: сайт без заданного цвета
  // показывал бы чужое имя и чужой логотип из кеша при живой CMS. Ровно этот
  // дефект уже стоил кабинету бренда целиком.
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  const brand = v2();
  brand.brand.primaryColor = null;

  assert.equal(window.Theming.applyBrand(brand), true);
  assert.equal(document.getElementById('brandName').textContent, 'Apex');
  assert.equal(markSrc(document), 'https://media/apex-dark.svg');
  assert.equal(
    document.documentElement.style.getPropertyValue('--accent').trim(),
    '',
    'цвета не было — акцент не трогаем, остаётся палитра платформы',
  );
});

test('мусор вместо ответа отвергается: раскрашиваться им нельзя', (t) => {
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  for (const junk of [null, 'строка', 42, {}, { contract: 'v1' }, { name: '' }]) {
    assert.equal(window.Theming.applyBrand(junk), false, `должно отвергаться: ${JSON.stringify(junk)}`);
  }

  assert.equal(document.getElementById('brandName').textContent, 'Exchange', 'имя не тронуто');
  assert.equal(document.querySelector('.logo__mark').textContent, 'E', 'знак не тронут');
});

test('плоский ответ без имени не опознаётся, даже если в нём есть цвет', (t) => {
  // Нашлось мутацией: снятие проверки «плоская форма обязана иметь имя» не
  // красило ничего, потому что её подпирала соседняя — «пригодного нет вовсе».
  // Две проверки на одном пути маскируют смерть одной из них, и разошлись они
  // ровно здесь: документ с одним только цветом перекрасил бы терминал, не
  // будучи брендом. У плоской формы имя — единственный опознавательный признак.
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  assert.equal(window.Theming.applyBrand({ primaryColor: '#ff0000' }), false);
  assert.equal(
    document.documentElement.style.getPropertyValue('--accent').trim(),
    '',
    'неопознанный документ не красит терминал',
  );
});

test('опознанный ответ без единого пригодного поля — не успех', (t) => {
  // Иначе пустышка легла бы в last-good и пережила бы перезагрузку.
  const { window, cleanup } = setup();
  t.after(cleanup);

  const empty = v2();
  empty.site.name = '';
  empty.brand.primaryColor = null;
  empty.brand.logoLight = null;
  empty.brand.logoDark = null;
  empty.brand.favicon = null;

  assert.equal(window.Theming.applyBrand(empty), false);
});

// ---- Логотип и тема --------------------------------------------------------

test('светлая тема берёт logoLight, тёмная — logoDark', (t) => {
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  window.Theming.setTheme('light');
  window.Theming.applyBrand(v2());
  assert.equal(markSrc(document), 'https://media/apex-light.svg');

  window.Theming.setTheme('dark');
  assert.equal(markSrc(document), 'https://media/apex-dark.svg', 'смена темы меняет логотип');
});

test('нет варианта для тёмной темы — имя текстом, а не светлый знак (Р-063)', (t) => {
  // Решение штаба против прежнего прочтения этой команды. `logoLight`
  // предназначен для светлого фона, то есть вероятно нарисован тёмными
  // чернилами: на тёмной теме он не «хуже тёмного варианта», а невидим —
  // вышло бы пустое место, про которое код считает, что логотип показан.
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  const brand = v2();
  brand.brand.logoDark = null;

  window.Theming.applyBrand(brand);

  assert.equal(markSrc(document), null, 'светлый знак на тёмный фон не ставим');
  assert.equal(document.querySelector('.logo__mark').textContent, 'A', 'знак уступает место инициалу');
  assert.equal(document.getElementById('brandName').textContent, 'Apex', 'полное имя рядом');
});

test('тот же бренд на светлой теме знак показывает', (t) => {
  // Зеркало предыдущего. Без этой пары мутация «не показывать логотип
  // никогда» осталась бы зелёной: отказ касается тёмного фона, а не логотипа.
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  const brand = v2();
  brand.brand.logoDark = null;

  window.Theming.setTheme('light');
  window.Theming.applyBrand(brand);

  assert.equal(markSrc(document), 'https://media/apex-light.svg');
});

test('переход на тёмную тему убирает светлый знак, а не оставляет висеть', (t) => {
  // Знак переиспользуется между перерисовками. Если бы смена темы его не
  // трогала, светлый логотип остался бы на тёмном фоне — то есть исчез бы с
  // экрана, оставив пустой квадрат.
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  const brand = v2();
  brand.brand.logoDark = null;

  window.Theming.setTheme('light');
  window.Theming.applyBrand(brand);
  assert.equal(markSrc(document), 'https://media/apex-light.svg');

  window.Theming.setTheme('dark');

  assert.equal(markSrc(document), null, 'картинка убрана');
  assert.equal(document.querySelector('.logo__mark').textContent, 'A');
});

test('та же картинка не пересоздаётся при смене темы', (t) => {
  // Сначала количество, потом тождество: сравнение «нашёлся элемент до и
  // после» зелено и при полном пересоздании.
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  const brand = v2();
  // Оба варианта — один файл: тогда смена темы не меняет адрес, и видно,
  // переиспользуется элемент или создаётся заново.
  brand.brand.logoDark = img('https://media/apex-light.svg', 'Apex');

  window.Theming.applyBrand(brand);
  const before = document.querySelectorAll('.logo__mark img');
  assert.equal(before.length, 1);

  window.Theming.setTheme('light');
  const after = document.querySelectorAll('.logo__mark img');

  assert.equal(after.length, 1, 'картинка ровно одна');
  assert.equal(after[0], before[0], 'тот же элемент, а не новый с тем же адресом');
});

test('фавикон не зависит от темы и берёт logoLight', (t) => {
  // Вкладка браузера про нашу тему ничего не знает. Оставить её без иконки
  // из-за тёмной темы терминала значило бы перенести наше ограничение туда,
  // где оно не действует.
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  window.Theming.applyBrand(v2());
  assert.equal(document.querySelector('link[rel="icon"]').href, 'https://media/apex.ico');

  const noFav = v2();
  noFav.brand.favicon = null;
  noFav.brand.logoDark = null;
  window.Theming.applyBrand(noFav); // тема тёмная, знака в шапке нет

  assert.equal(document.querySelector('link[rel="icon"]').href, 'https://media/apex-light.svg');
});

// ---- Адрес ресурса ---------------------------------------------------------

test('адрес v2: слаг подставляется в путь', (t) => {
  const { window, cleanup } = setup();
  t.after(cleanup);

  assert.equal(
    window.Theming.brandRequestUrl({
      brandUrl: 'http://cms:3000/v1/public/sites/{site}/brand',
      site: 'apex-ru',
    }),
    'http://cms:3000/v1/public/sites/apex-ru/brand',
  );
});

test('адрес v2 без слага не собирается вовсе', (t) => {
  // `/sites//brand` вернул бы чужой или никакой бренд: правдоподобный неверный
  // ответ вместо отказа.
  const { window, cleanup } = setup();
  t.after(cleanup);

  assert.equal(
    window.Theming.brandRequestUrl({ brandUrl: 'http://cms:3000/v1/public/sites/{site}/brand', site: '' }),
    null,
  );
});

test('адрес legacy: слаг и локаль параметрами', (t) => {
  const { window, cleanup } = setup();
  t.after(cleanup);

  assert.equal(
    window.Theming.brandRequestUrl({ brandUrl: 'http://cab:3002/v1/cms/brand/', site: 'apex-ru', locale: 'ru' }),
    'http://cab:3002/v1/cms/brand?site=apex-ru&locale=ru',
  );
});

test('пустой адрес — не ходим никуда', (t) => {
  const { window, cleanup } = setup();
  t.after(cleanup);

  assert.equal(window.Theming.brandRequestUrl({ brandUrl: '' }), null);
  assert.equal(window.Theming.brandRequestUrl({}), null);
});

// ---- Что этими тестами НЕ проверено ---------------------------------------

test('мутация «убрали проверку typeof raw» непокрасима — и это свойство, а не дыра', () => {
  // Снятие `typeof raw !== 'object'` в `normalizeBrand` не красит ни одного
  // теста, и покрасить его нечем: строка, число и массив всё равно отсеются
  // ниже — у них нет ни `name`, ни пары `site`/`brand`, а значит нет и
  // пригодного поля. Проверка стоит первой ради читаемости, а не ради защиты.
  //
  // Названо вслух, потому что следующий попробует «дозакрыть» этот пункт ради
  // полноты и напишет бесполезный тест.
  assert.ok(true);
});

test('мутация «ответ v2 не запрашивается по новому адресу» здесь непрогоняема', () => {
  // `fetchBrand` ходит в сеть, и подменять `fetch` значило бы проверить
  // подменённый `fetch`. Разница между адресами вынесена в чистую
  // `brandRequestUrl`, она покрыта выше, а связка «конфиг → запрос → применённый
  // бренд» закрывается только живым прогоном против CMS v2 — контур на неё ещё
  // не переехал. Записано здесь, чтобы следующий не решил, что связку забыли.
  assert.ok(true);
});
