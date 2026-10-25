#!/usr/bin/env python3
"""Разбор ответа реестра прогонов. OPS-36, зона `deploy`.

Печатает три числа через пробел: ВСЕГО ИДУТ УПАЛИ.
Имена упавших прогонов со ссылками уходят в stderr — чтобы в логе
гейта было видно, на что смотреть, а не только что всё плохо.

ПОЧЕМУ ОТДЕЛЬНЫМ ФАЙЛОМ, А НЕ СТРОКОЙ В КОНВЕЙЕРЕ. Его видно глазом,
и его можно прогнать на своей машине ровно тем кодом, какой пойдёт на
раннере. Первая редакция считала то же самое через `jq` в одну
строку — и на стенде, где `jq` не оказалось, три пустые строки
провалились в ветку «всё зелено»: сторож пропустил красный прогон.
Разбор, не давший чисел, обязан КРИЧАТЬ, а не возвращать нули.

ПОЧЕМУ python3, А НЕ jq. Не из вкуса: python3 есть и на раннере, и на
машине, где я пишу стенд. Инструмент, которого нет на стенде, делает
стенд проверкой другого кода.
"""

import json
import sys


def main() -> int:
    if len(sys.argv) != 2:
        print("нужен ровно один аргумент — файл с ответом реестра", file=sys.stderr)
        return 2

    try:
        with open(sys.argv[1], encoding="utf-8") as f:
            otvet = json.load(f)
    except (OSError, ValueError) as e:
        print("ответ реестра не прочитался и не разобрался: %s" % e, file=sys.stderr)
        return 3

    progony = otvet.get("workflow_runs")
    # Не `or []`: отсутствие списка — это НЕ пустой список. Первое
    # означает «реестр ответил не тем», второе — «прогонов нет», и
    # склеивать их нельзя, иначе сломанный ответ выглядит как чистый
    # коммит без проверок.
    if not isinstance(progony, list):
        print("в ответе нет списка прогонов", file=sys.stderr)
        return 3

    idut = [r for r in progony if r.get("status") != "completed"]
    upali = [
        r
        for r in progony
        if r.get("status") == "completed" and r.get("conclusion") != "success"
    ]

    for r in upali:
        print(
            "  %s #%s: %s  %s"
            % (r.get("name"), r.get("run_number"), r.get("conclusion"), r.get("html_url")),
            file=sys.stderr,
        )

    print("%d %d %d" % (len(progony), len(idut), len(upali)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
