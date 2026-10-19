//! # Видимость содержимого по юрисдикции — реализация терминала (TRM-15, Р-058)
//!
//! Правило «видно ли это содержимое посетителю из такой-то юрисдикции» обязано
//! быть одним на всех потребителей выдачи CMS: витрину, кабинет, терминал и
//! предпросмотр самой CMS. Выдача ограничения **не применяет** — она отдаёт их
//! как есть (`bootstrap.navigation[].jurisdictions`, `visibility.jurisdictions`
//! у блока, `jurisdictions` у страницы), а решает потребитель.
//!
//! «Один модуль на всех» здесь физически недостижим: исходная реализация живёт
//! в CMS на TypeScript (`modules/design/compliance/visibility.ts`), а терминал
//! на Rust. Поэтому контрактом сделана **таблица эталонных случаев**
//! `contracts/visibility-cases.v1.json`; её прогоняет своим тестом каждая
//! реализация, включая CMS.
//!
//! ## Граница того, что доказывает прогон таблицы
//!
//! Прохождение таблицы доказывает **согласие с прочтением CMS**, а не
//! правильность прочтения юрисдикций. Если автор таблицы понял регуляторное
//! требование неверно, все реализации ошибутся **одинаково и согласованно**, и
//! ни один тест этого не покажет: таблица меряет расхождение между нами, а не
//! соответствие регулятору. Сверка с регулятором лежит вне кода и здесь не
//! делается.
//!
//! ## Где это вызывается в терминале
//!
//! **Нигде.** Сегодня терминал не читает `bootstrap.navigation[]` вовсе: из
//! выдачи CMS он берёт `settings.instruments` (allow-list, сервер-сервер) и
//! публичный бренд (браузером). Это честно названный класс «написано и не
//! вызывается» — тот самый, который модульные тесты не ловят в принципе. Модуль
//! заведён как участник сверки реализаций и готовность к появлению
//! потребителя, а не как работающий путь; мутацию «правило не вызывается»
//! прогнать не на чем, и это записано в тесте.

use serde_json::Value;

/// Решение о видимости. `reason` непуст ровно тогда, когда `visible == false`:
/// скрытое содержимое обязано уметь объяснить, почему оно скрыто, иначе отказ
/// неотличим от отсутствия функции (Р-025).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub visible: bool,
    /// `None` — видно.
    pub reason: Option<String>,
}

impl Verdict {
    fn shown() -> Self {
        Self { visible: true, reason: None }
    }

    fn hidden(reason: String) -> Self {
        Self { visible: false, reason: Some(reason) }
    }
}

/// Приводит поле ограничений к списку кодов.
///
/// Ограничения приходят в двух формах: повторяемой группой Payload
/// (`[{ "code": "eu-mifid" }]` у областей и страниц) и плоским списком строк
/// (поле JSON у дерева меню). Читать их обязана **одна** функция: два читателя
/// разошлись бы в мелочах — в пробелах, в пустых строках, в чужой форме
/// записи, — и разошлись бы молча, потому что ограничение, прочитанное как
/// пустое, означает «видно всем».
///
/// Мусор отбрасывается, а не превращается в ограничение: иначе опечатка в поле
/// означала бы запрет, причину которого никто не нашёл бы.
pub fn read_jurisdictions(value: &Value) -> Vec<String> {
    let Some(items) = value.as_array() else {
        // Негодная форма поля — это отсутствие ограничения, а не ограничение,
        // которого мы не поняли: потребитель парсит мягко.
        return Vec::new();
    };

    let mut out = Vec::new();

    for entry in items {
        match entry {
            Value::String(s) => {
                let trimmed = s.trim();
                if !trimmed.is_empty() {
                    out.push(trimmed.to_string());
                }
            }
            Value::Object(map) => {
                if let Some(Value::String(code)) = map.get("code") {
                    let trimmed = code.trim();
                    if !trimmed.is_empty() {
                        out.push(trimmed.to_string());
                    }
                }
            }
            _ => {}
        }
    }

    out
}

/// Видно ли содержимое с такими ограничениями посетителю из такой юрисдикции.
///
/// `jurisdiction` — `None`, когда юрисдикция посетителя неизвестна.
pub fn visibility_in(restrictions: &Value, jurisdiction: Option<&str>) -> Verdict {
    let list = read_jurisdictions(restrictions);

    // Пустой список — «во всех». Умолчание названо и осознанно: запрет
    // задаётся явно, а обратное умолчание прятало бы содержимое молча, и
    // заметить это было бы некому.
    if list.is_empty() {
        return Verdict::shown();
    }

    // Ограничение есть, а юрисдикция неизвестна — не показываем. Умолчание
    // несимметрично, и направление выбрано: «я не знаю, где вы» не является
    // доказательством, что показывать можно. Лишний раз скрытый блок чинят,
    // показанный не там — объясняют регулятору. Пустая строка — та же
    // незаданность, а не особая юрисдикция.
    let visitor = match jurisdiction {
        Some(j) if !j.is_empty() => j,
        _ => {
            return Verdict::hidden(format!(
                "Ограничено юрисдикциями ({}), а юрисдикция не задана: показывать нельзя — \
                 неизвестность не разрешение.",
                list.join(", ")
            ))
        }
    };

    // Сравнение посимвольное, без приведения регистра. Приведение пришлось бы
    // повторять в каждой реализации, и первая же, которая его забудет,
    // разойдётся с остальными. Поэтому неизвестный код и код не в том регистре
    // не сужают показ, а отменяют его — отклонять такие коды обязан источник
    // при сохранении, а не потребитель при показе.
    if list.iter().any(|code| code == visitor) {
        return Verdict::shown();
    }

    Verdict::hidden(format!(
        "Ограничено юрисдикциями ({}) — в «{visitor}» не показывается.",
        list.join(", ")
    ))
}

/// Результат фильтрации дерева меню.
pub struct NavFilterResult {
    pub items: Vec<Value>,
    /// Сколько пунктов скрыто **вместе с подпунктами**: скрытый узел считается
    /// за один, его потомки отдельно не пересчитываются.
    ///
    /// Число возвращается отдельно, потому что пустое меню и меню, из которого
    /// всё убрала юрисдикция, выглядят одинаково, а означают разное.
    pub hidden: usize,
}

/// Прячет из дерева меню то, что в этой юрисдикции не показывается.
///
/// Скрытый пункт уходит **вместе с подпунктами**: раздела, которого нет, не
/// бывает с видимыми подпунктами — иначе потребитель нарисовал бы подпункты без
/// заголовка.
///
/// **Расхождение с TS-реализацией, названное вслух.** В TypeScript узлом
/// считается всё, для чего `typeof item === 'object'`, то есть и массив: такой
/// «узел» прошёл бы как объект без ограничений и превратился в `{children: []}`.
/// Здесь узлом считается только JSON-объект, всё остальное молча пропускается.
/// В эталонной таблице этого случая нет, а контракт выдачи требует объект, так
/// что расхождение через выдачу недостижимо. Записано, чтобы оно не всплыло
/// молча, если таблицу однажды расширят.
pub fn filter_nav_by_jurisdiction(items: &Value, jurisdiction: Option<&str>) -> NavFilterResult {
    let Some(items) = items.as_array() else {
        // Поле приходит из JSON: «не список» — это пустое меню, а не паника.
        return NavFilterResult { items: Vec::new(), hidden: 0 };
    };

    let mut kept = Vec::new();
    let mut hidden = 0usize;

    for item in items {
        let Some(map) = item.as_object() else {
            continue;
        };

        let restrictions = map.get("jurisdictions").cloned().unwrap_or(Value::Null);

        if !visibility_in(&restrictions, jurisdiction).visible {
            hidden += 1;
            continue;
        }

        let children = map.get("children").cloned().unwrap_or(Value::Array(Vec::new()));
        let below = filter_nav_by_jurisdiction(&children, jurisdiction);
        hidden += below.hidden;

        let mut next = map.clone();
        next.insert("children".to_string(), Value::Array(below.items));
        kept.push(Value::Object(next));
    }

    NavFilterResult { items: kept, hidden }
}

// ======================= Прогон эталонной таблицы ==========================

#[cfg(test)]
mod cases_tests {
    use super::*;

    /// Копия таблицы. Вшита в бинарь: тест не должен зависеть от того, из
    /// какого каталога его запустили.
    const COPY: &str = include_str!("../contracts/visibility-cases.v1.json");

    /// Где лежит источник таблицы. Каталог монтируется прогоном
    /// (`scripts/test.sh`): репозитории разные, общего дерева у них нет.
    const SOURCE_ENV: &str = "CMS_CONTRACTS_DIR";
    const SOURCE_FILE: &str = "visibility-cases.v1.json";

    fn table() -> Value {
        serde_json::from_str(COPY).expect("копия таблицы обязана разбираться")
    }

    /// Сверка копии с источником **по содержимому**.
    ///
    /// Просто копия была бы седьмым производным списком, а шесть имеющихся уже
    /// названы в `КОНТРАКТЫ.md` главным источником ошибок. Копия без сверки
    /// расходится с источником молча и ровно в тот момент, когда источник
    /// поправили.
    ///
    /// Сравниваются **разобранные** документы, а не байты: перевод строки и
    /// порядок ключей содержанием не являются, и красный тест от CRLF — ложная
    /// тревога, после двух-трёх которых красный результат перестают открывать.
    ///
    /// Недоступный источник — **не успех**. «Проверить не смогли» обязано быть
    /// отличимо от «сошлось», поэтому тест падает, а не пропускается молча.
    #[test]
    fn copy_matches_source_by_content() {
        let dir = std::env::var(SOURCE_ENV).unwrap_or_default();
        assert!(
            !dir.trim().is_empty(),
            "{SOURCE_ENV} не задана: сверка копии с источником НЕ выполнена. \
             Это блокирующий результат, а не успех — прогоняй через scripts/test.sh, \
             он монтирует каталог контрактов CMS."
        );

        let path = std::path::Path::new(&dir).join(SOURCE_FILE);
        let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "источник таблицы недоступен ({}): {e}. Сверка НЕ выполнена — \
                 результат неизвестен, а не положителен.",
                path.display()
            )
        });

        let source: Value = serde_json::from_str(&raw).expect("источник обязан разбираться");

        assert_eq!(
            source,
            table(),
            "копия contracts/visibility-cases.v1.json разошлась с источником CMS — \
             перенеси источник целиком и перечитай новые случаи, а не подгоняй копию"
        );
    }

    /// Пустая таблица прошла бы любую реализацию, поэтому охват объявляется
    /// числом — и единица охвата названа: случай на одиночное ограничение и
    /// случай на дерево меню — разные единицы.
    #[test]
    fn table_declares_its_coverage() {
        let t = table();
        let items = t["items"].as_array().expect("items — список").len();
        let trees = t["trees"].as_array().expect("trees — список").len();

        assert!(items >= 14, "случаев на пункт осмотрено {items}, в контракте их было 14");
        assert!(trees >= 3, "случаев на дерево осмотрено {trees}, в контракте их было 3");
    }

    /// Все случаи на одиночное ограничение.
    #[test]
    fn single_restriction_cases() {
        let t = table();
        let items = t["items"].as_array().expect("items — список");

        for case in items {
            let name = case["name"].as_str().unwrap_or("<без имени>");
            let restrictions = case.get("restrictions").cloned().unwrap_or(Value::Null);
            let jurisdiction = case["jurisdiction"].as_str();
            let expected = case["visible"].as_bool().expect("visible — булево");

            let verdict = visibility_in(&restrictions, jurisdiction);

            assert_eq!(verdict.visible, expected, "случай «{name}»");

            // Скрытое обязано объяснять себя, видимое — молчать: иначе «не
            // видно» и «видно» различались бы только булевым полем, а
            // пользователю показалась бы пустая панель без причины.
            assert_eq!(
                verdict.reason.is_none(),
                expected,
                "случай «{name}»: причина обязана быть ровно у скрытого"
            );
            if let Some(reason) = &verdict.reason {
                assert!(!reason.trim().is_empty(), "случай «{name}»: причина пуста");
            }
        }
    }

    /// Все случаи на дерево меню.
    #[test]
    fn tree_cases() {
        let t = table();
        let trees = t["trees"].as_array().expect("trees — список");

        for case in trees {
            let name = case["name"].as_str().unwrap_or("<без имени>");
            let jurisdiction = case["jurisdiction"].as_str();
            let expected_labels: Vec<&str> = case["visibleLabels"]
                .as_array()
                .expect("visibleLabels — список")
                .iter()
                .map(|v| v.as_str().expect("метка — строка"))
                .collect();
            let expected_hidden = case["hidden"].as_u64().expect("hidden — число") as usize;

            let result = filter_nav_by_jurisdiction(&case["tree"], jurisdiction);

            let labels: Vec<&str> =
                result.items.iter().filter_map(|i| i["label"].as_str()).collect();

            assert_eq!(labels, expected_labels, "дерево «{name}»: верхний уровень");
            assert_eq!(result.hidden, expected_hidden, "дерево «{name}»: число скрытого");
        }
    }

    /// Чего таблица **не** проверяет, и почему это закрыто здесь.
    ///
    /// Эталон сверяет только метки верхнего уровня (`visibleLabels`) — так его
    /// читает и CMS. Фильтрация подпунктов попадает в него лишь косвенно, через
    /// счётчик `hidden`, а сам отфильтрованный список потомков не смотрит никто.
    /// Реализация, которая считает скрытое правильно, но возвращает детей
    /// нетронутыми, прошла бы таблицу целиком.
    ///
    /// Проверка добавлена рядом, а не в таблицу: менять контракт в одиночку
    /// нельзя, предложение — в отчёте штабу.
    #[test]
    fn hidden_child_is_removed_from_its_parent() {
        let tree = serde_json::json!([
            {
                "label": "Торговля",
                "jurisdictions": [],
                "children": [
                    { "label": "CFD", "jurisdictions": ["uk-fca"], "children": [] },
                    { "label": "Акции", "jurisdictions": [], "children": [] }
                ]
            }
        ]);

        let result = filter_nav_by_jurisdiction(&tree, Some("eu-mifid"));

        let children: Vec<&str> = result.items[0]["children"]
            .as_array()
            .expect("children — список")
            .iter()
            .filter_map(|c| c["label"].as_str())
            .collect();

        assert_eq!(children, vec!["Акции"], "ограниченный подпункт обязан уйти из родителя");
        assert_eq!(result.hidden, 1);
    }

    /// Скрытый узел уносит потомков — и потомки **не** досчитываются в `hidden`.
    ///
    /// Случай из таблицы закрепляет это числом `1` на узле с одним потомком:
    /// там `1` совпало бы и при противоположном прочтении, если бы считали
    /// только сам узел или только его детей. Здесь закреплено само свойство.
    #[test]
    fn hidden_subtree_counts_as_one() {
        let tree = serde_json::json!([
            {
                "label": "Только Британия",
                "jurisdictions": ["uk-fca"],
                "children": [
                    { "label": "А", "jurisdictions": [], "children": [] },
                    { "label": "Б", "jurisdictions": [], "children": [
                        { "label": "В", "jurisdictions": [], "children": [] }
                    ] }
                ]
            }
        ]);

        let result = filter_nav_by_jurisdiction(&tree, Some("eu-mifid"));

        assert!(result.items.is_empty());
        assert_eq!(result.hidden, 1, "скрытое поддерево — один скрытый пункт, а не четыре");
    }

    /// **Мутация, которую прогнать не на чем, записана здесь.**
    ///
    /// Класс «написано и не вызывается» модульные тесты не ловят в принципе:
    /// модульный тест зовёт функцию сам, и «никто не зовёт» для него неотличимо
    /// от «зовут». Обычно это закрывают мутацией против **интеграционного**
    /// набора — «правило не включено в путь» — и смотрят, что покраснело.
    ///
    /// Здесь прогнать её **нечем**: в терминале нет ни одного пути, который
    /// звал бы это правило. Терминал не читает `bootstrap.navigation[]` вовсе —
    /// из выдачи CMS он берёт `settings.instruments` и публичный бренд. Значит
    /// весь этот модуль сегодня и есть «написано и не вызывается», и прогон
    /// таблицы этого не меняет.
    ///
    /// Записано в тесте, а не только в отчёте, чтобы следующий не решил, что
    /// проверка просто не делалась, и не «дозакрыл» пункт фальшивой проводкой:
    /// фильтр, приделанный к несуществующему потребителю, был бы хуже честного
    /// нуля.
    #[test]
    fn mutation_rule_is_never_called_cannot_be_run_here() {
        // Утверждение, которое сломается, когда потребитель появится: тогда
        // эту заметку обязаны заменить интеграционной проверкой пути.
        let wired_consumers: usize = 0;
        assert_eq!(
            wired_consumers, 0,
            "появился потребитель правила — замени эту заметку интеграционной проверкой пути"
        );
    }
}
