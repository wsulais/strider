#!/usr/bin/env python3
"""Validate citation *anchors* and ADR `refs`, which `govctl check` cannot.

`govctl check` verifies that a cited artifact exists. It does not verify:

  1. **item-level staleness** — `[[RFC-0007:C-EXTRACT]] 7` still resolves after
     C-EXTRACT is cut from 7 items to 4, because the clause exists. The anchor
     is dead and nothing notices.
  2. **bare cross-RFC citations** — WRITING.md fixes bare `C-NAME n` as meaning
     "within this RFC". A bare citation of a clause in *another* RFC is wrong,
     and is invisible to reference checking because a bare `C-NAME` is not an
     artifact id, so no unknown-reference diagnostic fires.
  3. **ADR `refs` drift** — an ADR citing a clause inline without listing it in
     `refs`. Hand-maintained refs drift silently.

Classes 1 and 2 are created by exactly the operation a growing corpus keeps
performing: moving clauses between RFCs. Class 3 needs no operation at all —
it accretes from writing.

Install per AUDITING.md: copy to `gov/guard/scripts/`, provision as
`GUARD-CITATION-ANCHORS-RESOLVE`. Run from the repo root.

Exit 1 on problems. Warnings print but do not fail — a `refs` entry that is
never cited inline may be a deliberate "informed by" link.
"""
import glob
import re
import sys
import tomllib
from pathlib import Path

NUM_ITEM = re.compile(r"^(\d+)\. ", re.M)
BRACKETED = re.compile(r"\[\[(RFC-\d{4}):(C-[A-Z0-9-]+)\]\](?:\s+(\d+(?:\s*(?:,|and)\s*\d+)*))?")
# a bare clause mention, not preceded by ':' (which would make it bracketed)
BARE = re.compile(r"(?<![:\w-])(C-[A-Z][A-Z0-9-]{2,})(?:\s+(\d+))?(?![-\w])")
ANY_ID = re.compile(r"\[\[((?:RFC|ADR)-\d{4}(?::C-[A-Z0-9-]+)?)\]\]")
BARE_ADR = re.compile(r"(?<![\[\w-])(ADR-\d{4})(?![-\w])")


def load():
    """(rfc, clause id) -> item count; clause id -> [owning rfcs]; informative ids."""
    items, owner, informative = {}, {}, set()
    for f in sorted(glob.glob("gov/rfc/RFC-*/clauses/*.toml")):
        rfc = f.split("/")[2]
        d = tomllib.load(open(f, "rb"))
        cid = d["govctl"]["id"]
        text = d["content"].get("text", "")
        nums = [int(n) for n in NUM_ITEM.findall(text)]
        items[(rfc, cid)] = max(nums) if nums else 0
        owner.setdefault(cid, []).append(rfc)
        # An informative clause (overview, intro, motivation) is named in prose
        # without citing an item. Derived from `kind`, so no per-project list.
        if d["govctl"].get("kind") == "informative":
            informative.add(cid)
    return items, owner, informative


def sources():
    for p in ("gov/rfc/*/clauses/*.toml", "gov/adr/*.toml", "gov/guard/*.toml"):
        for f in sorted(glob.glob(p)):
            yield Path(f)


def body_of(d, is_rfc):
    if is_rfc:
        return d["content"].get("text", "")
    content = d.get("content", {})
    body = "\n".join(str(content.get(k, "")) for k in ("context", "decision", "consequences"))
    for a in content.get("alternatives", []):
        body += "\n" + "\n".join(
            [a.get("text", ""), a.get("rejection_reason", "")] + a.get("pros", []) + a.get("cons", [])
        )
    return body


def main():
    items, owner, informative = load()
    problems, warnings = [], []

    for f in sources():
        d = tomllib.load(open(f, "rb"))
        is_rfc = f.parts[:2] == ("gov", "rfc")
        is_adr = f.parts[:2] == ("gov", "adr")
        own_rfc = f.parts[2] if is_rfc else None
        cid_self = d["govctl"]["id"]
        body = body_of(d, is_rfc)
        where = f"{own_rfc}:{cid_self}" if is_rfc else cid_self

        # 1. bracketed citations: does the clause exist, and does the item?
        for rfc, cid, nums in BRACKETED.findall(body):
            if (rfc, cid) not in items:
                problems.append(f"{where}: [[{rfc}:{cid}]] — clause not in {rfc}")
                continue
            hi = items[(rfc, cid)]
            for n in re.findall(r"\d+", nums or ""):
                if int(n) > hi:
                    problems.append(
                        f"{where}: [[{rfc}:{cid}]] {n} — clause has {hi} items (stale anchor)")

        # 2. bare citations must be same-RFC
        stripped = BRACKETED.sub(" ", body)
        for cid, n in BARE.findall(stripped):
            if cid == cid_self or cid in informative:
                continue
            homes = owner.get(cid)
            if not homes:
                continue
            if own_rfc is None:
                problems.append(f"{where}: bare '{cid}' — must use [[{homes[0]}:{cid}]]")
            elif own_rfc not in homes:
                problems.append(
                    f"{where}: bare '{cid}{' ' + n if n else ''}' — lives in "
                    f"{'/'.join(homes)}, needs [[{homes[0]}:{cid}]]")
            elif n and int(n) > items[(own_rfc, cid)]:
                problems.append(
                    f"{where}: '{cid} {n}' — clause has "
                    f"{items[(own_rfc, cid)]} items (stale anchor)")

        # 3. ADR refs are derived, not maintained
        if is_adr:
            refs = set(d["govctl"].get("refs", []))
            cited = set(ANY_ID.findall(body)) | set(BARE_ADR.findall(body))
            for c in sorted(cited - refs):
                problems.append(f"{where}: cites {c} inline but omits it from refs")
            for r in sorted(refs - cited):
                if not any(c.startswith(r + ":") for c in cited):
                    warnings.append(f"{where}: refs {r}, never cited in the body")

    for w in sorted(set(warnings)):
        print("  warn: " + w)
    if problems:
        print(f"{len(problems)} citation problem(s):")
        for p in sorted(set(problems)):
            print("  " + p)
        return 1
    print("all citation anchors resolve")
    return 0


if __name__ == "__main__":
    sys.exit(main())
