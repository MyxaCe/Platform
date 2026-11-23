#!/usr/bin/env python3
"""Отпечаток входов образа: всё, из чего Docker собирает образ.

Печатает sha256 от: версии этого скрипта, текста Dockerfile, цели сборки,
аргументов сборки (IMAGE_BUILD_ARGS) и содержимого всех файлов, которые
попадают в образ через COPY/ADD из контекста (с учётом .dockerignore).
Одинаковый отпечаток значит: сборка получит те же входы, готовый образ можно
переиспользовать под новым тегом вместо пересборки.

Если Dockerfile делает то, что скрипт оценить не умеет (ADD по URL, heredoc,
переменная в источнике, источник вне контекста), печатается `unsure` —
тогда образ собирается заново.

В отпечаток НЕ входят базовые образы (FROM node:22-alpine и т.п.): при
переиспользовании остаётся та базовая версия, с которой образ был собран.
Принудительно пересобрать всё: поменять переменную IMAGE_INPUTS_SALT.
"""
import argparse
import hashlib
import json
import os
import re
import shlex
import stat
import sys

VERSION = '1'


def logical_lines(text):
    buf = ''
    for raw in text.splitlines():
        line = raw.rstrip()
        if not buf and (not line.strip() or line.lstrip().startswith('#')):
            continue
        if line.endswith('\\'):
            buf += line[:-1] + ' '
            continue
        buf += line
        yield buf.strip()
        buf = ''
    if buf.strip():
        yield buf.strip()


def copy_sources(dockerfile_text):
    """Источники из контекста у всех COPY/ADD без --from во всех стадиях."""
    sources = []
    for line in logical_lines(dockerfile_text):
        m = re.match(r'^(COPY|ADD)\s+(.*)$', line, re.I)
        if not m:
            continue
        instr, rest = m.group(1).upper(), m.group(2).strip()
        if '<<' in rest:
            raise ValueError('heredoc')
        flags = []
        if rest.startswith('['):
            parts = json.loads(rest)
        else:
            tokens = shlex.split(rest, posix=True)
            flags = [p for p in tokens if p.startswith('--')]
            parts = [p for p in tokens if not p.startswith('--')]
        if any(f.startswith('--from') for f in flags):
            continue
        if len(parts) < 2:
            raise ValueError('COPY без назначения')
        for s in parts[:-1]:
            if instr == 'ADD' and re.match(r'^[a-z]+://', s, re.I):
                raise ValueError('ADD по URL')
            if '$' in s:
                raise ValueError('переменная в источнике')
            sources.append(s)
    return sources


def glob_regex(p):
    out = ''
    i = 0
    while i < len(p):
        if p.startswith('**/', i):
            out += '(?:.*/)?'
            i += 3
            continue
        if p.startswith('**', i):
            out += '.*'
            i += 2
            continue
        c = p[i]
        if c == '*':
            out += '[^/]*'
        elif c == '?':
            out += '[^/]'
        elif c == '[':
            j = p.find(']', i)
            if j < 0:
                out += re.escape(c)
            else:
                out += p[i:j + 1]
                i = j
        else:
            out += re.escape(c)
        i += 1
    return re.compile('^' + out + '$')


def ignore_rules(context):
    path = os.path.join(context, '.dockerignore')
    rules = []
    if not os.path.exists(path):
        return rules
    with open(path, encoding='utf-8') as f:
        for raw in f:
            p = raw.strip()
            if not p or p.startswith('#'):
                continue
            neg = p.startswith('!')
            if neg:
                p = p[1:].strip()
            if p.startswith('./'):
                p = p[2:]
            p = p.strip('/')
            rules.append((neg, glob_regex(p)))
    return rules


def ignored(rel, rules):
    """Правило действует на путь и на любой каталог над ним; последнее совпавшее побеждает."""
    parts = rel.split('/')
    prefixes = ['/'.join(parts[:k]) for k in range(1, len(parts) + 1)]
    result = False
    for neg, rx in rules:
        if any(rx.match(pfx) for pfx in prefixes):
            result = not neg
    return result


def walk(context, src, rules):
    s = src
    if s.startswith('./'):
        s = s[2:]
    s = s.strip('/') or '.'
    if any(ch in s for ch in '*?['):
        rx = glob_regex(s)
        candidates = []
        for root, dirs, files in os.walk(context):
            for name in dirs + files:
                rel = os.path.relpath(os.path.join(root, name), context).replace(os.sep, '/')
                if rx.match(rel):
                    candidates.append(rel)
        if not candidates:
            raise ValueError(f'шаблон ничего не нашёл: {src}')
    else:
        candidates = [s]
    out = []
    ctx = os.path.normpath(context)
    for c in candidates:
        full = os.path.normpath(os.path.join(context, c))
        if full != ctx and not full.startswith(ctx + os.sep):
            raise ValueError('источник вне контекста')
        if os.path.isdir(full) and not os.path.islink(full):
            for root, dirs, files in os.walk(full):
                for name in files + [d for d in dirs if os.path.islink(os.path.join(root, d))]:
                    rel = os.path.relpath(os.path.join(root, name), context).replace(os.sep, '/')
                    if not ignored(rel, rules):
                        out.append(rel)
        elif os.path.lexists(full):
            rel = os.path.relpath(full, context).replace(os.sep, '/')
            if not ignored(rel, rules):
                out.append(rel)
        else:
            raise ValueError(f'источник не найден: {c}')
    return out


def file_digest(path):
    st = os.lstat(path)
    if stat.S_ISLNK(st.st_mode):
        return 'link:' + os.readlink(path)
    h = hashlib.sha256()
    with open(path, 'rb') as f:
        for chunk in iter(lambda: f.read(1 << 20), b''):
            h.update(chunk)
    return ('x' if st.st_mode & 0o111 else '-') + h.hexdigest()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument('--context', required=True)
    ap.add_argument('--file', required=True)
    ap.add_argument('--target', default='')
    ap.add_argument('--list', action='store_true', help='печатать файлы-входы вместо отпечатка')
    a = ap.parse_args()
    context = os.path.abspath(a.context)
    with open(a.file, 'rb') as f:
        dockerfile = f.read()
    try:
        rules = ignore_rules(context)
        files = set()
        for src in copy_sources(dockerfile.decode('utf-8')):
            files.update(walk(context, src, rules))
    except (ValueError, json.JSONDecodeError, OSError) as e:
        print(f'image-inputs: входы не оценить ({e}) — собирать заново', file=sys.stderr)
        print('unsure')
        return
    if a.list:
        for rel in sorted(files):
            print(rel)
        return
    h = hashlib.sha256()
    h.update(f'v{VERSION}\0salt={os.environ.get("IMAGE_INPUTS_SALT", "")}\0'.encode())
    h.update(b'dockerfile\0' + dockerfile + b'\0')
    h.update(f'target\0{a.target}\0'.encode())
    h.update(b'args\0' + os.environ.get('IMAGE_BUILD_ARGS', '').strip().encode() + b'\0')
    for rel in sorted(files):
        h.update(f'{rel}\0{file_digest(os.path.join(context, rel))}\0'.encode())
    print(h.hexdigest())


if __name__ == '__main__':
    main()
