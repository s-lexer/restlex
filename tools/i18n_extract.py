"""Собирает все русские строки из restlex.py для перевода.

    python tools/i18n_extract.py            -> i18n/strings_ru.json

Обычные строки — как есть (плюс отдельные строки многострочных текстов),
f-строки — шаблонами с {0}, {1}… на месте подстановок.
"""
import ast, json, re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CYR = re.compile("[А-Яа-яЁё]")


def template(node: ast.JoinedStr):
    out, n = [], 0
    for v in node.values:
        if isinstance(v, ast.Constant):
            out.append(str(v.value).replace("{", "{{").replace("}", "}}"))
        else:
            out.append("{%d}" % n)
            n += 1
    return "".join(out)


def collect(src: str):
    tree = ast.parse(src)
    exact, patterns = set(), set()
    in_fstring = set()
    for node in ast.walk(tree):
        if isinstance(node, ast.JoinedStr):
            for v in node.values:
                in_fstring.add(id(v))
            t = template(node)
            if CYR.search(t):
                patterns.add(t)
    for node in ast.walk(tree):
        if isinstance(node, ast.Constant) and isinstance(node.value, str) and id(node) not in in_fstring:
            s = node.value
            if CYR.search(s):
                exact.add(s)
                if "\n" in s:
                    exact.update(x for x in s.split("\n") if CYR.search(x))
    return sorted(exact), sorted(patterns)


if __name__ == "__main__":
    exact, patterns = collect((ROOT / "restlex.py").read_text(encoding="utf-8"))
    out = ROOT / "i18n" / "strings_ru.json"
    out.parent.mkdir(exist_ok=True)
    out.write_text(json.dumps({"exact": exact, "patterns": patterns}, ensure_ascii=False, indent=1),
                   encoding="utf-8")
    print(f"exact={len(exact)} patterns={len(patterns)} chars={sum(map(len, exact + patterns))} -> {out}")
