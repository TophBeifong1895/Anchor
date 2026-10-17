#!/usr/bin/env python3
"""Reproduce P's syntax-point census; read-only source inspection, no dependencies.

Old scopes are deliberately the existing operation envelope (including Session,
Call and assistant guards). They are not a claim of feature parity or deletability.
Each row includes shared helpers; `union` de-duplicates by source position.
"""
import bisect
import json
import re
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
BASE = "a234cb6"
NEW = "ff09f2c"
R = "rust/anchor-runtime/src/graph/"
H = "rust/anchor-runner-host/src/"
S = R + "scheduler.rs"
K = H + "application/kernel_runs.rs"
C = H + "application/control.rs"
A = H + "application/abandon.rs"
U = H + "application/recovery.rs"
V = H + "application/run_projection.rs"
E = H + "execution.rs"
G = R + "runner.rs"
P = R + "parallel.rs"

# Inclusive, reviewable source ranges. New paths are fixed to the probe commit;
# changes that move these ranges require updating and reviewing this census.
SCOPES = {
    "execution": {
        "old": [(G,31,765),(G,961,981),(E,283,305)],
        "new": [(S,14,24),(S,32,52),(S,64,190),(K,29,38),(K,108,134),(K,303,320)],
    },
    "stop": {
        "old": [(C,35,171),(C,220,243),(G,68,77),(G,258,262),(G,371,378),(G,581,590),(P,66,76)],
        "new": [(C,32,34),(K,135,172),(S,32,52)],
    },
    "resume": {
        "old": [(C,35,195),(C,208,252),(U,159,176),(G,31,67)],
        "new": [(C,32,34),(K,135,172),(K,321,354),(S,32,52)],
    },
    "abandon": {
        "old": [(A,72,82),(A,124,317)],
        "new": [(A,72,82),(A,191,199),(K,135,172),(S,32,52)],
    },
    "unknown_recovery": {
        "old": [(U,6,231),(C,173,195),(G,603,614)],
        "new": [(U,14,16),(S,83,96),(S,157,165),(S,192,218)],
    },
    "restart": {
        "old": [(G,31,67),(G,102,266),(G,303,319)],
        "new": [(S,64,119),(K,59,93),(K,108,134),(K,321,354)],
    },
    "projection": {
        "old": [(V,28,108),(C,220,252)],
        "new": [(V,33,39),(K,173,233),(H+'application/queries.rs',49,82)],
    },
}
# Strip comments and literals without changing offsets/newlines. Raw strings,
# normal strings, chars and nested block comments are handled; lifetimes remain.
TOKEN = re.compile(r'//[^\n]*|/\*|r(#+)?"|"|\'(?:\\.|[^\'\\\n])\'')

def mask_source(text, literals=True):
    chars = list(text)
    pos = 0
    while m := TOKEN.search(text, pos):
        start = m.start()
        tag = m.group()
        if tag == '/*':
            depth, end = 1, m.end()
            while depth:
                n = re.search(r'/\*|\*/', text[end:])
                if n is None:
                    raise ValueError('unterminated block comment')
                end += n.end()
                depth += 1 if n.group() == '/*' else -1
        elif tag.startswith('r'):
            terminator = '"' + (m.group(1) or '')
            end = text.index(terminator, m.end()) + len(terminator)
        elif tag == '"':
            end = m.end()
            while end < len(text):
                if text[end] == '\\':
                    end += 2
                elif text[end] == '"':
                    end += 1
                    break
                else:
                    end += 1
        else:
            end = m.end()
        if literals or tag.startswith('//') or tag == '/*':
            for i in range(start, end):
                if chars[i] != '\n':
                    chars[i] = ' '
        pos = end
    return ''.join(chars)

PATTERNS = {
    'if': r'\bif\b',  # if-let, else-if, guards and expression-if each count once
    'arm': r'=>',      # every match arm, including Phase/Reason/port outcomes
    'field_write': r'\b[A-Za-z_]\w*(?:\s*\.\s*[A-Za-z_]\w*)+\s*(?:[+\-*/]?=(?!=|>))',
}

def read(version, path):
    revision = BASE if version == 'old' else NEW
    return subprocess.check_output(['git','show',f'{revision}:{path}'],cwd=ROOT,text=True)

CACHE = {}
def census(version, spans):
    selected = {}
    for path, start, end in spans:
        if (version,path) not in CACHE:
            source = read(version,path)
            CACHE[version,path] = (source.splitlines(),mask_source(source).splitlines(),mask_source(source,False).splitlines())
        original, masked, _ = CACHE[version,path]
        assert end <= len(original), (path,end,len(original))
        for line in range(start, end+1):
            selected[path,line] = (original[line-1],masked[line-1])
    hits = []
    counts = dict.fromkeys(PATTERNS,0)
    code_lines = sum(bool(CACHE[version,p][2][i-1].strip()) for p,i in selected)
    for path in sorted({p for p,_ in selected}):
        original, masked, _ = CACHE[version,path]
        text = '\n'.join(masked)
        starts = [0] + [m.end() for m in re.finditer('\n', text)]
        for kind,pattern in PATTERNS.items():
            for m in re.finditer(pattern,text):
                line = bisect.bisect_right(starts,m.start())
                if (path,line) in selected:
                    counts[kind] += 1
                    hits.append(dict(file=path,line=line,kind=kind,column=m.start()-starts[line-1]+1,source=original[line-1].strip()))
    return dict(points=sum(counts.values()),counts=counts,physical_lines=len(selected),
                code_lines=code_lines,scopes=[f'{p}:{a}-{b}' for p,a,b in spans],hits=hits)

# Added glue is added source lines in the selected scopes, excluding blank or
# comment-only lines; existing shared parse_reason is not counted as new glue.
def added_lines():
    result = set()
    paths = sorted({p for row in SCOPES.values() for p,_,_ in row['new']})
    for path in paths:
        if subprocess.run(['git','cat-file','-e',f'{BASE}:{path}'],cwd=ROOT,stderr=subprocess.DEVNULL).returncode:
            result.update((path,i) for i in range(1,len(read('new',path).splitlines())+1))
            continue
        diff = subprocess.check_output(['git','diff',BASE,NEW,'--unified=0','--',path],cwd=ROOT,text=True)
        line = 0
        for text in diff.splitlines():
            if text.startswith('@@'):
                line = int(re.search(r'\+(\d+)',text).group(1))
            elif text.startswith('+++') or text.startswith('---'):
                continue
            elif text.startswith('+'):
                result.add((path,line)); line += 1
            elif not text.startswith('-') and not text.startswith('\\'):
                line += 1
    return result

def main():
    additions = added_lines()
    result = dict(baseline=BASE,implementation=NEW,method='I + M + W syntax points; overlapping operation scopes',operations={})
    union = {'old':[],'new':[]}
    for operation,row in SCOPES.items():
        result['operations'][operation] = {}
        for version,spans in row.items():
            union[version].extend(spans)
            entry = census(version,spans)
            if version == 'new':
                keys = {(p,i) for p,a,b in spans for i in range(a,b+1)}
                entry['added_glue_lines'] = sum(bool(CACHE['new',p][2][i-1].strip()) for p,i in keys & additions)
            result['operations'][operation][version] = entry
    result['union'] = {v:census(v,spans) for v,spans in union.items()}
    keys = {(p,i) for p,a,b in union['new'] for i in range(a,b+1)}
    result['union']['new']['added_glue_lines'] = sum(bool(CACHE['new',p][2][i-1].strip()) for p,i in keys & additions)
    for name,row in result['operations'].items():
        print(name,row['old']['points'],row['new']['points'],row['new']['added_glue_lines'])
    print('union',[(v,r['points'],r['code_lines']) for v,r in result['union'].items()])
    result['capacity_sources'] = {}
    for path in [G,P,C,A,U]:
        text = read('old',path).split('#[cfg(test)]')[0]
        result['capacity_sources'][path] = dict(first_line=1,last_line=len(text.splitlines()),code_lines=sum(bool(l.strip()) for l in mask_source(text,False).splitlines()))
    print('capacity',result['capacity_sources'])
    print('union new glue', result['union']['new']['added_glue_lines'])
    (Path(__file__).parent/'counts.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')

if __name__ == '__main__':
    main()
