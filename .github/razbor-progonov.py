#!/usr/bin/env python3

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
