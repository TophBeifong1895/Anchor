#!/usr/bin/env python3
"""Count physical Rust diff lines by crate at immutable Git revisions.

No rename detection: moving code between files counts as removal + addition.
Inline cfg(test) items and files reached through test-only modules are tests.
Cargo manifests/lockfiles, docs, generated fixtures and Web are not Rust lines.
"""
import bisect
from collections import defaultdict
import json
from pathlib import Path
import re
import subprocess

HERE = Path(__file__).resolve().parent
ROOT = HERE.parents[2]
BASE = json.loads((HERE / "baseline.json").read_text())


def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT).decode()


def mask(source):
    # Retain newlines and character offsets while masking literals/comments.
    result = list(source)
    token = re.compile(r'//[^\n]*|/\*|(?:br|r)(?P<hash>\#*)"|b?"|b?\x27(?:\\.|[^\x27\\\n])\x27')
    pos = 0
    while match := token.search(source, pos):
        start, tag = match.start(), match.group()
        end = match.end()
        if tag == "/*":
            depth = 1
            while depth:
                nested = re.search(r"/\*|\*/", source[end:])
                if not nested:
                    raise ValueError("unterminated block comment")
                end += nested.end()
                depth += 1 if nested.group() == "/*" else -1
        elif tag.startswith(("r", "br")):
            end = source.index('"' + match.group("hash"), end) + 1 + len(match.group("hash"))
        elif tag in ('"', 'b"'):
            while end < len(source):
                if source[end] == "\\":
                    end += 2
                elif source[end] == '"':
                    end += 1
                    break
                else:
                    end += 1
        for i in range(start, end):
            if result[i] != "\n":
                result[i] = " "
        pos = end
    return "".join(result)


def test_partition(revision):
    names = [p for p in git("ls-tree", "-r", "--name-only", revision, "rust").splitlines() if p.endswith(".rs")]
    sources = {p: git("show", f"{revision}:{p}") for p in names}
    masks = {p: mask(s) for p, s in sources.items()}
    whole = {p for p in names if "/tests/" in p or "/benches/" in p or p.endswith("/tests.rs") or "/test_support/" in p or p.endswith("/test_support.rs")}
    spans = defaultdict(set)
    external = []
    for path, source in sources.items():
        masked = masks[path]
        starts = [0] + [m.end() for m in re.finditer("\n", source)]
        for attr in re.finditer(r"#\s*\[\s*cfg\s*\([^]]*\)\s*\]", masked):
            guard = attr.group()
            # Exact test or all(test, platform...) requires the test build.
            # any(test, feature="test-fixtures") stays production unless the
            # whole file is explicitly classified as shared test_support.
            if not (re.fullmatch(r"#\s*\[\s*cfg\s*\(\s*test\s*\)\s*\]", guard)
                    or re.match(r"#\s*\[\s*cfg\s*\(\s*all\s*\(\s*test\s*[,)]", guard)):
                continue
            following = masked[attr.end():]
            field = re.match(r"\s*(?:pub(?:\([^)]*\))?\s+)?(?:r#)?\w+\s*:(?!:)", following)
            brace = masked.find("{", attr.end())
            semi = masked.find(";", attr.end())
            stop = min(x for x in (brace, semi) if x >= 0)
            if field:
                # cfg(test) can guard just one struct field/initializer. Do
                # not mark the following production fields or methods as tests.
                end = attr.end() + field.end()
                stack = []
                while end < len(masked):
                    char = masked[end]
                    if char in "([{<":
                        stack.append(char)
                    elif stack and char in ")]}>":
                        stack.pop()
                    elif char == "," and not stack:
                        end += 1
                        break
                    end += 1
            elif stop == brace:
                depth, end = 1, brace + 1
                while depth:
                    if masked[end] == "{":
                        depth += 1
                    elif masked[end] == "}":
                        depth -= 1
                    end += 1
            else:
                end = semi + 1
            lo, hi = bisect.bisect_right(starts, attr.start()), bisect.bisect_right(starts, end - 1)
            spans[path].update(range(lo, hi + 1))
            if not field and stop == semi:
                item = source[attr.end():end]
                mod = re.search(r"\bmod\s+(\w+)\s*;", item)
                if mod:
                    location = re.search(r'#\[path\s*=\s*"([^"]+)"\]', item)
                    external.append((path, mod.group(1), location.group(1) if location else None))
    def resolve(path, name, explicit=None):
        parent = Path(path).parent
        candidates = [parent / explicit] if explicit else [parent / name / "mod.rs", parent / (name + ".rs"), parent / Path(path).stem / name / "mod.rs", parent / Path(path).stem / (name + ".rs")]
        return [str(p) for p in candidates if str(p) in sources]
    for path, name, explicit in external:
        whole.update(resolve(path, name, explicit))
    # Test-only external files can load test helpers whose names do not say tests.
    pending = list(whole)
    while pending:
        path = pending.pop()
        for mod in re.finditer(r'(?:(#\[path\s*=\s*"([^"]+)"\])\s*)?(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*;', sources[path]):
            for child in resolve(path, mod.group(3), mod.group(2)):
                if child not in whole:
                    whole.add(child)
                    pending.append(child)
        for include in re.finditer(r'include!\s*\(\s*"([^"]+)"\s*\)', sources[path]):
            child = str(Path(path).parent / include.group(1))
            if child in sources and child not in whole:
                whole.add(child)
                pending.append(child)
    return sources, whole, spans


def main():
    old, new = BASE["line_baseline"], BASE["after"]
    partitions = {rev: test_partition(rev) for rev in [old, new]}
    rows = defaultdict(lambda: {"production_added": 0, "production_deleted": 0, "test_added": 0, "test_deleted": 0})
    for rev in (old, new):
        for path in partitions[rev][0]:
            rows[path.split("/")[1]]
    files = {}
    for path in sorted(set(partitions[old][0]) | set(partitions[new][0])):
        diff = git("diff", "--no-renames", "--unified=0", old, new, "--", path)
        if not diff:
            continue
        crate = path.split("/")[1]
        counts = {k: 0 for k in rows[crate]}
        oi = ni = 0
        for line in diff.splitlines():
            if line.startswith("@@"):
                hunk = re.match(r"@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@", line)
                oi, ni = map(int, hunk.groups())
            elif line.startswith(("---", "+++", "\\")):
                continue
            elif line.startswith(("+", "-")):
                added = line[0] == "+"
                _, whole, spans = partitions[new if added else old]
                test = path in whole or (ni if added else oi) in spans[path]
                key = ("test" if test else "production") + ("_added" if added else "_deleted")
                counts[key] += 1
                if added:
                    ni += 1
                else:
                    oi += 1
            elif line.startswith(" "):
                oi += 1
                ni += 1
        files[path] = counts
        for k, v in counts.items():
            rows[crate][k] += v
    unclassified = []
    for rev, (sources, whole, spans) in partitions.items():
        for path, source in sources.items():
            if path in whole:
                continue
            for line, text in enumerate(mask(source).splitlines(), 1):
                if re.search(r"#\s*\[\s*(?:tokio::)?test\s*(?:\(|\])", text) and line not in spans[path]:
                    unclassified.append(f"{rev}:{path}:{line}")
    assert not unclassified, unclassified
    out = {"baseline": old, "after": new, "method": __doc__, "crates": dict(sorted(rows.items())), "files": files, "totals": {k: sum(r[k] for r in rows.values()) for k in next(iter(rows.values()))}, "unclassified_test_attributes": unclassified, "test_only_files": {rev: sorted(parts[1]) for rev, parts in partitions.items()}, "limitations": ["Physical lines include whitespace and comments in their owning section; not SLOC", "Mixed production/test files are split at cfg(test) item boundaries; benches and test_support are test infrastructure", "Rust examples and build.rs count as production; Cargo metadata is excluded", "No feature execution is implied; module reachability is a source classification"]}
    (HERE / "line-counts.json").write_text(json.dumps(out, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"crates": out["crates"], "totals": out["totals"]}, indent=2))


if __name__ == "__main__":
    main()
