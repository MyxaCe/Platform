use serde_json::Value;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefusalCode {
    JurisdictionUnknown,
    JurisdictionNotListed,
}

impl RefusalCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::JurisdictionUnknown => "jurisdiction-unknown",
            Self::JurisdictionNotListed => "jurisdiction-not-listed",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verdict {
    pub visible: bool,
    pub reason: Option<RefusalCode>,
    pub explanation: Option<String>,
}

impl Verdict {
    fn shown() -> Self {
        Self {
            visible: true,
            reason: None,
            explanation: None,
        }
    }

    fn hidden(reason: RefusalCode, explanation: String) -> Self {
        Self {
            visible: false,
            reason: Some(reason),
            explanation: Some(explanation),
        }
    }
}

pub fn read_jurisdictions(value: &Value) -> Vec<String> {
    let Some(items) = value.as_array() else {
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

pub fn visibility_in(restrictions: &Value, jurisdiction: Option<&str>) -> Verdict {
    let list = read_jurisdictions(restrictions);

    if list.is_empty() {
        return Verdict::shown();
    }

    let visitor = match jurisdiction {
        Some(j) if !j.is_empty() => j,
        _ => {
            return Verdict::hidden(
                RefusalCode::JurisdictionUnknown,
                format!(
                    "Ограничено юрисдикциями ({}), а юрисдикция не задана: показывать нельзя — \
                     неизвестность не разрешение.",
                    list.join(", ")
                ),
            )
        }
    };

    if list.iter().any(|code| code == visitor) {
        return Verdict::shown();
    }

    Verdict::hidden(
        RefusalCode::JurisdictionNotListed,
        format!(
            "Ограничено юрисдикциями ({}) — в «{visitor}» не показывается.",
            list.join(", ")
        ),
    )
}

pub struct NavFilterResult {
    pub items: Vec<Value>,
    pub hidden: usize,
}

pub fn filter_nav_by_jurisdiction(items: &Value, jurisdiction: Option<&str>) -> NavFilterResult {
    let Some(items) = items.as_array() else {
        return NavFilterResult {
            items: Vec::new(),
            hidden: 0,
        };
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

        let children = map
            .get("children")
            .cloned()
            .unwrap_or(Value::Array(Vec::new()));
        let below = filter_nav_by_jurisdiction(&children, jurisdiction);
        hidden += below.hidden;

        let mut next = map.clone();
        next.insert("children".to_string(), Value::Array(below.items));
        kept.push(Value::Object(next));
    }

    NavFilterResult {
        items: kept,
        hidden,
    }
}

#[cfg(test)]
mod cases_tests {
    use super::*;

    const COPY: &str = include_str!("../contracts/visibility-cases.v2.json");

    const SOURCE_ENV: &str = "CMS_CONTRACTS_DIR";
    const SOURCE_FILE: &str = "visibility-cases.v2.json";

    fn table() -> Value {
        serde_json::from_str(COPY).expect("копия таблицы обязана разбираться")
    }

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

    #[test]
    fn table_version_is_the_one_we_checked_against() {
        let t = table();
        let id = t["$id"].as_str().unwrap_or_default();

        assert!(
            id.ends_with("/v2/visibility-cases.json"),
            "таблица больше не v2 ({id}) — перенеси источник целиком, перечитай новые              случаи и проверь мутациями, не осталось ли рядом проверок, которые таблица              уже содержит (Р-061)"
        );
    }

    #[test]
    fn table_declares_its_coverage() {
        let t = table();
        let items = t["items"].as_array().expect("items — список").len();
        let trees = t["trees"].as_array().expect("trees — список").len();

        assert!(
            items >= 19,
            "случаев на пункт осмотрено {items}, в v2 их было 19"
        );
        assert!(
            trees >= 6,
            "случаев на дерево осмотрено {trees}, в v2 их было 6"
        );
    }

    #[test]
    fn single_restriction_cases() {
        let t = table();
        let items = t["items"].as_array().expect("items — список");

        for case in items {
            let name = case["name"].as_str().unwrap_or("<без имени>");
            let restrictions = case.get("restrictions").cloned().unwrap_or(Value::Null);
            let jurisdiction = case["jurisdiction"].as_str();
            let expected = case["visible"].as_bool().expect("visible — булево");
            let expected_code = case["reason"].as_str();

            let verdict = visibility_in(&restrictions, jurisdiction);

            assert_eq!(verdict.visible, expected, "случай «{name}»");
            assert_eq!(
                verdict.reason.map(RefusalCode::as_str),
                expected_code,
                "случай «{name}»: машинный код отказа"
            );

            assert_eq!(
                verdict.explanation.is_none(),
                expected,
                "случай «{name}»: подпись обязана быть ровно у скрытого"
            );
            if let Some(text) = &verdict.explanation {
                assert!(!text.trim().is_empty(), "случай «{name}»: подпись пуста");
            }
        }
    }

    #[test]
    fn tree_cases() {
        fn shape(items: &[Value]) -> Value {
            Value::Array(
                items
                    .iter()
                    .map(|i| {
                        let kids = i["children"].as_array().cloned().unwrap_or_default();
                        serde_json::json!({ "label": i["label"], "children": shape(&kids) })
                    })
                    .collect(),
            )
        }

        let t = table();
        let trees = t["trees"].as_array().expect("trees — список");

        for case in trees {
            let name = case["name"].as_str().unwrap_or("<без имени>");
            let jurisdiction = case["jurisdiction"].as_str();
            let expected_hidden = case["hidden"].as_u64().expect("hidden — число") as usize;

            let result = filter_nav_by_jurisdiction(&case["tree"], jurisdiction);

            assert_eq!(shape(&result.items), case["visibleTree"], "дерево «{name}»");
            assert_eq!(
                result.hidden, expected_hidden,
                "дерево «{name}»: число скрытого"
            );
        }
    }

    #[test]
    fn mutation_rule_is_never_called_cannot_be_run_here() {
        let wired_consumers: usize = 0;
        assert_eq!(
            wired_consumers, 0,
            "появился потребитель правила — замени эту заметку интеграционной проверкой пути"
        );
    }
}
