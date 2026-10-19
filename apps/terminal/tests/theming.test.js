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

test('legacy: плоский ответ читается по-прежнему', (t) => {
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

test('нет варианта для тёмной темы — показываем основной, а не пустоту', (t) => {
  // Контракт запрещает выдумывать тёмный вариант за владельца бренда, но
  // `logoLight` там назван «основным логотипом», а тема терминала по
  // умолчанию тёмная: правило «нет logoDark — логотипа нет» оставило бы без
  // логотипа большинство посетителей.
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  const brand = v2();
  brand.brand.logoDark = null;

  window.Theming.applyBrand(brand);

  assert.equal(markSrc(document), 'https://media/apex-light.svg');
});

test('та же картинка не пересоздаётся при смене темы', (t) => {
  // Сначала количество, потом тождество: сравнение «нашёлся элемент до и
  // после» зелено и при полном пересоздании.
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  const brand = v2();
  brand.brand.logoDark = null; // в обеих темах логотип один и тот же

  window.Theming.applyBrand(brand);
  const before = document.querySelectorAll('.logo__mark img');
  assert.equal(before.length, 1);

  window.Theming.setTheme('light');
  const after = document.querySelectorAll('.logo__mark img');

  assert.equal(after.length, 1, 'картинка ровно одна');
  assert.equal(after[0], before[0], 'тот же элемент, а не новый с тем же адресом');
});

test('фавикон берётся из favicon, а без него — из логотипа', (t) => {
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  window.Theming.applyBrand(v2());
  assert.equal(document.querySelector('link[rel="icon"]').href, 'https://media/apex.ico');

  const noFav = v2();
  noFav.brand.favicon = null;
  window.Theming.applyBrand(noFav);
  assert.equal(document.querySelector('link[rel="icon"]').href, 'https://media/apex-dark.svg');
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

test('мутация «ответ v2 не запрашивается по новому адресу» здесь непрогоняема', () => {
  // `fetchBrand` ходит в сеть, и подменять `fetch` значило бы проверить
  // подменённый `fetch`. Разница между адресами вынесена в чистую
  // `brandRequestUrl`, она покрыта выше, а связка «конфиг → запрос → применённый
  // бренд» закрывается только живым прогоном против CMS v2 — контур на неё ещё
  // не переехал. Записано здесь, чтобы следующий не решил, что связку забыли.
  assert.ok(true);
});
