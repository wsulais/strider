#!/usr/bin/env python3
"""Measure RFC cohesion against coupling, and fail on the corpus smell.

An RFC is a unit holding one subject whose clauses cite each other more than they cite
outward. That is measurable rather than assertable: build the clause-to-clause citation
graph, group by owning RFC, and compare intra-group citations against cross-group ones.

This guard fails on ONE condition — the whole-corpus ratio falling below 1.00, meaning
cross-RFC citations have come to outnumber intra-RFC ones and the boundaries cut through a
densely connected spec. Per-RFC figures are reported but never fail, because a low ratio on
one group is ambiguous: it can mean the group is not a subject, or it can mean the group's
clauses have honest dependencies on another RFC. Distinguishing those needs a reader.

Run: python3 gov/guard/scripts/check-rfc-boundaries.py
"""
import glob, re, sys, tomllib
from collections import defaultdict

FLOOR = 1.00

owner, texts = {}, {}
for path in sorted(glob.glob('gov/rfc/*/clauses/*.toml')):
    try:
        d = tomllib.load(open(path, 'rb'))
    except Exception as e:                      # a malformed clause is check's business
        print(f"  skipped {path}: {e}")
        continue
    if 'govctl' not in d or 'content' not in d:
        continue
    rid = path.split('/')[2]
    cid = d['govctl']['id']
    if cid == 'C-OVERVIEW':                     # recurs per RFC, informative, uncited
        continue
    owner[cid] = rid
    texts[(rid, cid)] = d['content'].get('text', '')

edges = defaultdict(list)
for (rid, cid), t in texts.items():
    for _, tgt in re.findall(r'\[\[(RFC-\d+):(C-[A-Z-]+)\]\]', t):
        edges[cid].append(tgt)
    for m in re.findall(r'(?<!:)\b(C-[A-Z-]+)\b', re.sub(r'\[\[[^\]]+\]\]', ' ', t)):
        if m in owner and m != cid:             # a bare id resolves within its own RFC
            edges[cid].append(m)

intra = defaultdict(int)
cross = defaultdict(int)
for src, targets in edges.items():
    if src not in owner:
        continue
    for tgt in targets:
        if tgt not in owner:
            continue
        (intra if owner[tgt] == owner[src] else cross)[owner[src]] += 1

ti = sum(intra.values())
tc = sum(cross.values())
print(f"  {'RFC':10} {'intra':>6} {'cross':>6}   ratio")
for rid in sorted(set(list(intra) + list(cross))):
    i, c = intra[rid], cross[rid]
    r = i / c if c else float('inf')
    print(f"  {rid:10} {i:6} {c:6}   {r:.2f}")
ratio = ti / tc if tc else float('inf')
print(f"  {'TOTAL':10} {ti:6} {tc:6}   {ratio:.2f}   (floor {FLOOR:.2f})")

if ratio < FLOOR:
    print(f"\nFAIL: cross-RFC citations outnumber intra-RFC ones ({ratio:.2f} < {FLOOR:.2f}).")
    print("The boundaries cut through a densely connected spec. Measure candidate")
    print("regroupings before moving anything: a correct split raises cohesion on BOTH")
    print("sides of the seam, and one that lowers it anywhere is the wrong split.")
    sys.exit(1)

print("\nOK: intra-RFC citations still outnumber cross-RFC ones.")
