#!/usr/bin/env python3
"""Чистка пакетов ghcr.io после ускорения CI (ТД-018).

Что удаляется (только при DRY_RUN=0; по умолчанию — сухой прогон):
  * пакеты `<образ>-cache`: версии, у которых ВСЕ теги `buildcache-<ветка>`, старше 14 дней
    (тег `buildcache` основной ветки не трогается);
  * пакеты образов: версии без тегов старше 7 дней, если на их digest не ссылается
    ни одна сохраняемая версия (индекс/список манифестов, мультиплатформа, аттестации);
  * версии, у которых ВСЕ теги `inputs-<отпечаток>`, старше 30 дней (без тега-хеша коммита).

Что не удаляется никогда:
  * версия с любым тегом из 40 hex-символов (хеш коммита — это то, что выкатывается и откатывается);
  * всё, что моложе MIN_AGE_DAYS (30 дней) — нижняя граница поверх правил выше;
  * дочерние манифесты сохраняемых версий;
  * всё в пакете, по которому не удалось прочитать список или манифесты (при сомнении — оставить).

Возраст версии — по updated_at (последнее изменение, в т.ч. новый тег), это осторожнее created_at.
Только стандартная библиотека Python. Токен — GITHUB_TOKEN workflow (packages: write).
"""
from __future__ import annotations

import base64
import concurrent.futures as cf
import datetime as dt
import json
import os
import re
import sys
import urllib.error
import urllib.parse
import urllib.request

API = "https://api.github.com"
REGISTRY = "https://ghcr.io"
SHA_TAG = re.compile(r"^[0-9a-f]{40}$")
INDEX_TYPES = {
    "application/vnd.oci.image.index.v1+json",
    "application/vnd.docker.distribution.manifest.list.v2+json",
}
ACCEPT = ", ".join(
    [
        "application/vnd.oci.image.index.v1+json",
        "application/vnd.docker.distribution.manifest.list.v2+json",
        "application/vnd.oci.image.manifest.v1+json",
        "application/vnd.docker.distribution.manifest.v2+json",
    ]
)

TOKEN = os.environ["GITHUB_TOKEN"]
ACTOR = os.environ.get("GITHUB_ACTOR", "github-actions")
OWNER = os.environ["PACKAGE_OWNER"]
OWNER_KIND = "orgs" if os.environ.get("PACKAGE_OWNER_TYPE", "User").lower() == "organization" else "users"
PACKAGES = [p for p in os.environ.get("PACKAGES", "").split() if p]
DRY_RUN = os.environ.get("DRY_RUN", "1") != "0"
MIN_AGE_DAYS = int(os.environ.get("MIN_AGE_DAYS", "30"))
UNTAGGED_DAYS = int(os.environ.get("UNTAGGED_DAYS", "7"))
CACHE_BRANCH_DAYS = int(os.environ.get("CACHE_BRANCH_DAYS", "14"))
INPUTS_DAYS = int(os.environ.get("INPUTS_DAYS", "30"))
NOW = dt.datetime.now(dt.timezone.utc)


class HttpError(Exception):
    def __init__(self, status: int, body: str):
        super().__init__(f"HTTP {status}: {body[:300]}")
        self.status = status


def request(url: str, *, method: str = "GET", headers: dict | None = None) -> tuple[bytes, dict]:
    req = urllib.request.Request(url, method=method, headers=headers or {})
    try:
        with urllib.request.urlopen(req, timeout=60) as resp:
            return resp.read(), dict(resp.headers)
    except urllib.error.HTTPError as e:
        raise HttpError(e.code, e.read().decode("utf-8", "replace")) from None


def gh(path: str, method: str = "GET") -> tuple[object, dict]:
    body, hdrs = request(
        API + path,
        method=method,
        headers={
            "Authorization": f"Bearer {TOKEN}",
            "Accept": "application/vnd.github+json",
            "X-GitHub-Api-Version": "2022-11-28",
        },
    )
    return (json.loads(body) if body else None), hdrs


def list_versions(pkg: str) -> list[dict]:
    out: list[dict] = []
    page = 1
    while True:
        data, _ = gh(f"/{OWNER_KIND}/{OWNER}/packages/container/{urllib.parse.quote(pkg)}/versions?per_page=100&page={page}")
        assert isinstance(data, list)
        out.extend(data)
        if len(data) < 100:
            return out
        page += 1


def registry_token(pkg: str) -> str:
    basic = base64.b64encode(f"{ACTOR}:{TOKEN}".encode()).decode()
    scope = urllib.parse.quote(f"repository:{OWNER.lower()}/{pkg}:pull")
    body, _ = request(f"{REGISTRY}/token?service=ghcr.io&scope={scope}", headers={"Authorization": f"Basic {basic}"})
    return json.loads(body)["token"]


def fetch_manifest(pkg: str, token: str, digest: str) -> dict:
    body, hdrs = request(
        f"{REGISTRY}/v2/{OWNER.lower()}/{pkg}/manifests/{digest}",
        headers={"Authorization": f"Bearer {token}", "Accept": ACCEPT},
    )
    doc = json.loads(body)
    media = doc.get("mediaType") or hdrs.get("Content-Type", "")
    children = [m["digest"] for m in doc.get("manifests", []) if "digest" in m] if media in INDEX_TYPES or "manifests" in doc else []
    size = sum(int(x.get("size", 0)) for x in doc.get("layers", []))
    size += int((doc.get("config") or {}).get("size", 0))
    return {"media": media, "children": children, "size": size}


def age_days(v: dict) -> float:
    stamp = v.get("updated_at") or v.get("created_at")
    t = dt.datetime.fromisoformat(stamp.replace("Z", "+00:00"))
    return (NOW - t).total_seconds() / 86400


def rule_for(pkg: str, v: dict, age: float) -> str | None:
    """Какое правило удаления подходит БЕЗ учёта нижней границы и ссылок (None — оставить)."""
    tags = v["metadata"]["container"]["tags"]
    if any(SHA_TAG.match(t) for t in tags):
        return None
    if pkg.endswith("-cache"):
        if tags and all(t.startswith("buildcache-") for t in tags):
            return "cache-branch" if age >= CACHE_BRANCH_DAYS else None
        if tags:
            return None  # buildcache основной ветки и всё прочее с тегами
        return "untagged" if age >= UNTAGGED_DAYS else None
    if not tags:
        return "untagged" if age >= UNTAGGED_DAYS else None
    if all(t.startswith("inputs-") for t in tags):
        return "inputs-only" if age >= INPUTS_DAYS else None
    return None


def human(n: int) -> str:
    for unit in ("Б", "КБ", "МБ", "ГБ"):
        if n < 1024:
            return f"{n:.0f} {unit}"
        n /= 1024
    return f"{n:.1f} ТБ"


def process(pkg: str) -> dict:
    res = {"pkg": pkg, "error": None, "total": 0, "sha": 0, "fresh": 0, "referenced": 0,
           "delete": [], "delete_no_floor": 0, "deleted": 0, "delete_errors": []}
    try:
        versions = list_versions(pkg)
    except HttpError as e:
        res["error"] = f"список версий: HTTP {e.status}"
        res["status"] = e.status
        return res
    res["total"] = len(versions)
    try:
        token = registry_token(pkg)
        with cf.ThreadPoolExecutor(max_workers=8) as ex:
            manifests = dict(zip((v["name"] for v in versions), ex.map(lambda v: fetch_manifest(pkg, token, v["name"]), versions)))
    except Exception as e:  # noqa: BLE001 — при любом сомнении пакет не трогаем
        res["error"] = f"манифесты: {e}"
        return res

    decision: dict[int, str | None] = {}
    for v in versions:
        age = age_days(v)
        rule = rule_for(pkg, v, age)
        tags = v["metadata"]["container"]["tags"]
        if any(SHA_TAG.match(t) for t in tags):
            res["sha"] += 1
        if rule:
            res["delete_no_floor"] += 1
        if rule and age < MIN_AGE_DAYS:
            res["fresh"] += 1
            rule = None
        decision[v["id"]] = rule

    # Ссылки: дети любой сохраняемой версии сохраняются (до неподвижной точки).
    by_digest = {v["name"]: v for v in versions}
    while True:
        keep_children: set[str] = set()
        for v in versions:
            if decision[v["id"]] is None:
                keep_children.update(manifests[v["name"]]["children"])
        changed = False
        for d in keep_children:
            v = by_digest.get(d)
            if v and decision[v["id"]] is not None:
                decision[v["id"]] = None
                res["referenced"] += 1
                changed = True
        if not changed:
            break

    for v in versions:
        rule = decision[v["id"]]
        if rule:
            res["delete"].append({
                "id": v["id"], "digest": v["name"], "rule": rule,
                "tags": v["metadata"]["container"]["tags"], "age": round(age_days(v), 1),
                "size": manifests[v["name"]]["size"],
            })

    if not DRY_RUN:
        # Сначала родители (индексы), потом дети — чтобы не оставить индекс с висячей ссылкой.
        order = sorted(res["delete"], key=lambda x: 0 if manifests[x["digest"]]["children"] else 1)
        for item in order:
            try:
                gh(f"/{OWNER_KIND}/{OWNER}/packages/container/{urllib.parse.quote(pkg)}/versions/{item['id']}", method="DELETE")
                res["deleted"] += 1
            except HttpError as e:
                res["delete_errors"].append(f"{item['digest'][:19]}: HTTP {e.status}")
    return res


def main() -> int:
    if not PACKAGES:
        print("PACKAGES пуст — нечего чистить", file=sys.stderr)
        return 1
    mode = "СУХОЙ ПРОГОН (ничего не удаляется)" if DRY_RUN else "УДАЛЕНИЕ"
    lines = [
        f"## Чистка реестра ghcr.io — {mode}",
        "",
        f"Нижняя граница: моложе {MIN_AGE_DAYS} дн. не удаляется ничего. Правила: без тегов ≥{UNTAGGED_DAYS} дн. и без ссылок; "
        f"`buildcache-<ветка>` ≥{CACHE_BRANCH_DAYS} дн.; только `inputs-*` ≥{INPUTS_DAYS} дн. Тег-хеш коммита — не удаляется никогда.",
        "",
        "| Пакет | Версий | С хешем коммита | Под удаление | Объём* | Без границы 30 дн. | Сохранено ссылкой | Удалено | Ошибки |",
        "|---|---|---|---|---|---|---|---|---|",
    ]
    details: list[str] = []
    forbidden = False
    total_del = total_bytes = 0
    for pkg in PACKAGES:
        r = process(pkg)
        if r.get("status") in (401, 403):
            forbidden = True
        size = sum(x["size"] for x in r["delete"])
        total_del += len(r["delete"])
        total_bytes += size
        errs = "; ".join(filter(None, [r["error"], *r["delete_errors"]])) or "—"
        lines.append(
            f"| `{pkg}` | {r['total']} | {r['sha']} | {len(r['delete'])} | {human(size)} | {r['delete_no_floor']} | "
            f"{r['referenced']} | {r['deleted']} | {errs} |"
        )
        for x in r["delete"][:15]:
            tags = ", ".join(x["tags"]) or "без тегов"
            details.append(f"- `{pkg}` {x['digest'][:19]}… ({tags}) — {x['rule']}, {x['age']} дн., {human(x['size'])}")
        if len(r["delete"]) > 15:
            details.append(f"- `{pkg}`: ещё {len(r['delete']) - 15}")
    lines += ["", f"Итого под удаление: {total_del} версий, ~{human(total_bytes)}.",
              "", "*Объём — сумма слоёв манифестов; общие слои считаются в каждом, реальное освобождение меньше."]
    if details:
        lines += ["", "### Что удаляется", *details]
    if forbidden:
        lines += ["", "**Нет доступа к части пакетов (401/403).** В настройках пакета на GitHub: Package settings → "
                  "Manage Actions access → добавить этот репозиторий с ролью Admin."]
    text = "\n".join(lines)
    print(text)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as f:
            f.write(text + "\n")
    return 1 if forbidden else 0


if __name__ == "__main__":
    sys.exit(main())
