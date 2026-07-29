# Governance conventions

House rules for Strider's govctl artifacts. Precedence, highest first:

1. **This file** — Strider-specific additions only.
2. **`grill-gov`** — `WRITING.md` (clause and ADR text shape), `ROUTING.md` (what
   goes where), `AUDITING.md` (corpus-level checks), `SKILL.md` (govctl hazards).
3. **`rfc-writer` / `adr-writer`** — fields, CLI verbs, lifecycle, renderer-owned structure.

This file is deliberately short. Most of what this project worked out has since been
folded into `grill-gov` itself, and duplicating it here would be worse than useless:
holding the top precedence slot, a stale copy would silently **shadow** later upstream
improvements. That is the same divergence hazard `ROUTING.md` warns about, applied to
guidance rather than to specs.

**Read the skill for:** clause shape and the untestable-obligation patterns, malformed
negatives, sizes and grow-don't-fold, titles, refusal-half/capability-half (`WRITING.md`);
the ADR bar and the both-halves duplication test (`ROUTING.md`); citation integrity, the
RFC-boundary cohesion/coupling measurement, and numbering (`AUDITING.md`); the authority
direction, `render`-defaults, dangling-clause and `C-OVERVIEW` hazards (`SKILL.md`).

---

## 1. Verification lives with the claim

Each property's verification obligation appends to the clause that makes the claim,
rather than collecting in a central conformance clause:

| claim | verification |
|---|---|
| bounded memory | `RFC-0002:C-MEMORY` 8 |
| halo correctness | `RFC-0002:C-HALO` 4 |
| projection transparency | `RFC-0002:C-PROJECTION` 3 |
| reproducibility | `RFC-0003:C-COMMIT` 6 |
| invalidation cost bounds | `RFC-0007:C-INVALIDATION` 5 |

A central clause collecting these would break the pattern for the properties it
collects while leaving it intact for the rest, so two places would end up defining how
conformance is established. What genuinely belongs centrally is only the *claim* rule —
a release must not assert an unverified property — plus definitions no single clause
owns. That split is `RFC-0001:C-CONFORMANCE` (the rule) and `RFC-0002:C-VERIFIED` (what
a verified operator is).

`grill-gov`'s corollary — a scaling or bounding claim needs a paired verification
obligation — is the general form of this. The convention here is the *placement*.

## 2. Clauses per RFC: 4–10

`WRITING.md` bounds obligations per clause (~3–8) but not clauses per RFC. This corpus
targets **4–10**. Below four, the subject probably belongs inside a neighbour; the
front-door RFC at 6 and the operator contract at 10 are the working extremes.

Reached by measurement, not preference — see `AUDITING.md` for the method. What that
measurement produced here: an 11-clause RFC covering rendering, editing and extension
scored cohesion 1 on its extension group against 11 outward citations, which is what
"not a subject" looks like. Splitting it raised cohesion on both sides of each seam.

## 3. Where this repo knowingly diverges from its own measurement

`RFC-0006:C-LAYERING` items 1–2 prohibit a general-purpose query engine beneath the
library crates. That is a data-layer constraint, and by subject it belongs with the
operator contract; only item 3 is a rendering concern. It sits in the rendering RFC
because splitting a three-item clause across two RFCs costs more than the misfit does.

`RFC-0006:C-OVERVIEW` states this as a judgement rather than claiming the items are
rendering concerns. Recorded here so the next person measuring boundaries does not
"discover" it as a defect.

## 4. Local guard set

Beyond `GUARD-CITATION-ANCHORS-RESOLVE` (installed from
`grill-gov/scripts/check-citations.py` — **do not fork it locally**; re-copy on skill
update):

| guard | enforces |
|---|---|
| `GUARD-PORTABILITY-TARGET-COMPILES` | `RFC-0004:C-PORT-GATE` 1 |
| `GUARD-NO-AMBIENT-CAPABILITY-IN-LIBRARY-CRATES` | `RFC-0004:C-PORT-GATE` 2 |
| `GUARD-LIBRARY-LICENCE-COMPATIBILITY` | `RFC-0001:C-LICENSE` 4 |
| `GUARD-NO-QUERY-ENGINE-UNDER-CORE` | `RFC-0006:C-LAYERING` 1 |
| `GUARD-NO-TOOLKIT-UNDER-LIBRARY-CRATES` | `RFC-0006:C-TOOLKIT` 1 |
| `GUARD-SPEC-CITED-IN-TESTS` | the test→clause citation convention |

Held **out** of `default_guards` until a halo-declaring operator lands, because a guard
that always fails trains everyone to ignore guard output:
`GUARD-BOUNDED-MEMORY-DOES-NOT-SCALE-WITH-INPUT`,
`GUARD-HALO-CORRECTNESS-AGAINST-SINGLE-PARTITION-REFERENCE`,
`GUARD-COMMITTED-RUN-IS-BIT-IDENTICAL-ON-REPETITION`. **Promoting them is the moment the
first release's exit criteria become real** — forgetting removes the gate silently.

Guard commands wrap in `devenv shell --`, since guards run outside the devenv shell.

## 5. `source_scan` covers prose, not just code

`gov/config.toml` scans `CONTEXT.md` and `docs/*.md` as well as `crates/**/*.rs` and the
manifests. Without it, roughly thirty clause citations in the glossary were validated by
nothing and would have rotted through the renumbering silently.
