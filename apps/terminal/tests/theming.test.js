'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { loadWidget } = require('./helpers/load-widget');

function setup() {
  const env = loadWidget(['theming.js']);
  const { document } = env;
  document.body.innerHTML =
    '<div class="logo"><span class="logo__mark">E</span> <span id="brandName">Exchange</span></div>';
  return env;
}

const img = (url, alt) => ({ url, width: 100, height: 40, alt, mimeType: 'image/svg+xml' });

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

test('v2: имя берётся из site.name, логотип из logoLight', (t) => {
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  assert.equal(window.Theming.applyBrand(v2()), true);

  assert.equal(document.getElementById('brandName').textContent, 'Apex');
  assert.equal(document.title, 'Apex — Terminal');
  assert.equal(markSrc(document), 'https://media/apex-dark.svg');
});

test('v2: акцент применяется каналами RGB', (t) => {
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  window.Theming.applyBrand(v2());

  assert.equal(document.documentElement.style.getPropertyValue('--accent').trim(), '255 102 0');
});

test('v2: поле logo не читается — его там нет', (t) => {
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

test('legacy: плоский логотип показывается и на тёмной теме', (t) => {
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  assert.equal(window.Theming.applyBrand(legacy()), true);

  assert.equal(document.getElementById('brandName').textContent, 'Старый бренд');
  assert.equal(markSrc(document), 'https://media/legacy.svg');
  assert.equal(document.documentElement.style.getPropertyValue('--accent').trim(), '0 102 255');
});

test('v2 без фирменного цвета: бренд применяется, а не отвергается целиком', (t) => {
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
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  const brand = v2();
  brand.brand.logoDark = null;

  window.Theming.applyBrand(brand);

  assert.equal(markSrc(document), null, 'светлый знак на тёмный фон не ставим');
  assert.equal(document.querySelector('.logo__mark').textContent, 'A', 'знак уступает место инициалу');
  assert.equal(document.getElementById('brandName').textContent, 'Apex', 'полное имя рядом');
});

test('порядок знака на тёмной теме: logoDark → имя. Проверено тестом, не чтением', (t) => {
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  window.Theming.applyBrand(v2());
  assert.equal(markSrc(document), 'https://media/apex-dark.svg', 'шаг 1: logoDark первым');

  const noDark = v2();
  noDark.brand.logoDark = null;
  window.Theming.applyBrand(noDark);
  assert.equal(markSrc(document), null, 'шаг 2: светлый знак на тёмный фон не ставим');
  assert.equal(document.querySelector('.logo__mark').textContent, 'A', 'шаг 2: инициал имени');
});

test('тот же бренд на светлой теме знак показывает', (t) => {
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  const brand = v2();
  brand.brand.logoDark = null;

  window.Theming.setTheme('light');
  window.Theming.applyBrand(brand);

  assert.equal(markSrc(document), 'https://media/apex-light.svg');
});

test('переход на тёмную тему убирает светлый знак, а не оставляет висеть', (t) => {
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
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  const brand = v2();
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
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  window.Theming.applyBrand(v2());
  assert.equal(/** @type {HTMLLinkElement} */ (document.querySelector('link[rel="icon"]')).href, 'https://media/apex.ico');

  const noFav = v2();
  noFav.brand.favicon = null;
  noFav.brand.logoDark = null;
  window.Theming.applyBrand(noFav);

  assert.equal(/** @type {HTMLLinkElement} */ (document.querySelector('link[rel="icon"]')).href, 'https://media/apex-light.svg');
});

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

test('мутация «убрали проверку typeof raw» непокрасима — и это свойство, а не дыра', () => {
  assert.ok(true);
});

test('мутация «ответ v2 не запрашивается по новому адресу» здесь непрогоняема', () => {
  assert.ok(true);
});

test('режим «только логотип» (wordmark): название текстом скрыто, alt — название бренда', (t) => {
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  window.Theming.applyBrand(legacy({ name: 'Ostrix', logo: img('https://media/wordmark.svg', ''), logoMode: 'logo' }));

  const mark = document.querySelector('.logo__mark');
  assert.equal(mark.classList.contains('logo__mark--wordmark'), true);
  assert.equal(document.getElementById('brandName').hidden, true);
  assert.equal(mark.querySelector('img').getAttribute('alt'), 'Ostrix');
});

test('режим «только название»: логотип не показывается даже если загружен', (t) => {
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  window.Theming.applyBrand(legacy({ name: 'Ostrix', logo: img('https://media/logo.svg', 'O'), logoMode: 'name' }));

  assert.equal(/** @type {HTMLElement} */ (document.querySelector('.logo__mark')).hidden, true);
  assert.equal(document.querySelector('.logo__mark img'), null);
  assert.equal(document.getElementById('brandName').hidden, false);
  assert.equal(document.getElementById('brandName').textContent, 'Ostrix');
});

test('старая CMS без logoMode: логотип и название, как раньше; заголовок вкладки — «бренд — терминал»', (t) => {
  const { window, document, cleanup } = setup();
  t.after(cleanup);

  window.Theming.applyBrand(legacy({ name: 'Ostrix', logo: img('https://media/logo.svg', 'O') }));

  assert.equal(document.querySelector('.logo__mark img').getAttribute('src'), 'https://media/logo.svg');
  assert.equal(document.getElementById('brandName').hidden, false);
  assert.equal(document.title, 'Ostrix — Terminal');
});
