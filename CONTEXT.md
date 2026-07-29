# Domain Context — ubiquitous language

**Project: Strider** — a Rust + Qt6 workbench and crate ecosystem for out-of-core
point cloud processing. (Named for the gait: you *stride over* terrain rather than
sink into it, which is what out-of-core processing does to a dataset. Naming
analysis: `docs/naming/pointcloud-naming-2026-07-29.md`. Fallback candidate was
PointFlow.)

The shared vocabulary for the workbench. **Definitions only — zero obligations.**
Normative rules live in `gov/rfc/` clauses; the *why* behind big choices lives in
`gov/adr/`. Where a definition here and a clause disagree, the clause governs
(`RFC-0001:C-CONFORMANCE` 1) and this file is wrong.

How those artifacts are written is the `grill-gov` skill; what this repo adds on top
is `docs/governance-conventions.md`.

Scope today: **forestry LiDAR**, and one delivery target (desktop). The model is
kept domain- and format-generic — COPC is one source adapter among several, and
nothing in the vocabulary presumes trees — so bathymetry, urban survey, and
industrial scanning can be added without reshaping it.

## Core premise
- **Out-of-core** — processing a dataset larger than available memory by reading
  and discarding bounded portions of it, never materialising the whole. Strider's
  reason to exist. Contrast **in-core**, where the cloud is loaded once and held
  as an object: the model CloudCompare and its generation assume.
- **Bounded memory** — the property that an operation's peak memory is a function
  of its working-set size, not of the dataset's point count. The claim the whole
  project rests on (`RFC-0002:C-MEMORY`).
- **Streaming** — used only for *how data moves* (a stream of batches). Never used
  to mean out-of-core; a viewer can stream without processing out-of-core, which
  is precisely the QGIS/CloudCompare gap Strider fills.

## Data
- **Point** — one measurement: a position plus attributes. Addressable only by
  *point reference*, and only where that is permitted.
- **Batch** — an Arrow record batch of points, columnar. The unit of data movement
  between operators. Always **plain Arrow**, never wrapped
  (`RFC-0002:C-EXEC` 3).
- **Attribute** — one column of a batch. Chosen over *field* (Arrow's word for the
  schema entry) and *dimension* (PDAL's word for the same idea), both of which are
  ambiguous here. *Dimension* is reserved for spatial axes.
- **Node** — a cell of a source's spatial hierarchy; in practice an octree node of
  a COPC. Nodes exist in the **source**; partitions exist in the **pipeline**.
- **Source** — anything the spatial access layer can read: local or remote COPC,
  Parquet/GeoParquet, E57, LAS/LAZ. Formats are adapters; none is privileged
  internally.

## Execution
- **Spatial access layer** — turns a spatial request (a volume, at a level of
  detail) into a stream of batches by traversing a source's index. Owns index
  traversal, the node cache, and level of detail. Knows nothing of operators. The
  renderer talks to it directly (`RFC-0006:C-RENDER`).
- **Operator** — one processing step: crop, reproject, estimate normals, classify
  ground. Consumes batches, produces batches, and **declares what it needs**
  before running. Not called a *filter* (which in PDAL means only the middle of a
  pipeline) nor an *algorithm* (which names the mathematics, not the schedulable
  unit).
- **Requirement** — what an operator states it needs from its input, before
  executing: partitioning, minimum halo width, attributes read, pass count. The
  planner's whole job (`RFC-0002:C-EXEC` 1).
- **Planner** — reads requirements, inserts the repartitioning, halo construction
  or summary passes that satisfy them, and refuses to run a pipeline it cannot
  satisfy. **Not a query optimiser**: it satisfies requirements, it does not
  reorder for cost. Drift toward relational algebra here is the documented failure
  mode of `ADR-0002`. Remains a *single* subject even where part of an evaluation is
  delegated: a delegated consumer's stated requirements are an **input** to the
  planner's computation, never a second planner. Nothing outside it satisfies a
  Strider requirement — which is what lets satisfaction be required to *survive* to
  execution (`RFC-0002:C-EXEC` 5), an invariant that could not be stated if the
  satisfier varied by requirement.
- **Pipeline** — a composed sequence of operators, plus the source it reads.
- **Stage** — one position in a planned pipeline. A stream's spatial properties —
  partition bounds, level of detail, source node identity, halo width — belong to
  the *stage*, not to the batches flowing through it (`RFC-0002:C-EXEC` 4).
- **Partition bounds** — the volume a partition is *responsible for*, declared by whoever
  assembled it. Deliberately not the extent of the points a stage carries: halo points lie
  outside the bounds by construction, so bounds computed from the points held would enclose
  the halo and make every point a core point (`RFC-0002:C-EXEC` 4).
- **Partition** — the unit of work the planner hands an operator: a bounded region
  of space at one level of detail. Usually aligned to source nodes, not required
  to be. A *node* is what the file has; a *partition* is what the planner decided
  to process together.
- **Pass** — one traversal of the input by an operator. Two-pass operators (build a
  summary, then apply it) are the shape of morphological ground filters and of
  height above ground.
- **Summary** — the result of reducing an input: a minimum-elevation raster, a
  histogram, a decimated index. *Not* automatically small: whether its size is bounded
  independently of the input is the question `RFC-0002:C-SUMMARY` 1 makes an operator
  answer, and a gridded summary over a whole extent fails it. Reduce-then-apply is
  admissible out-of-core only where the summary is bounded, which for a grid means
  built per partition (`RFC-0002:C-SUMMARY` 3).

## Neighbourhoods
- **Halo** — points supplied to a partition from *outside* its bounds, so a
  neighbourhood computation near the boundary sees the neighbours a whole-dataset
  run would have given it. Called *ghost points* in the scientific-computing
  literature; Strider says **halo** for the region and **halo point** for a member,
  and avoids "ghost" as a noun.
- **Halo width** — the distance beyond a partition's bounds from which halo points
  are drawn. Declared by the operator, supplied by the planner. Under-declaring it
  is a correctness bug, not a performance bug (`RFC-0002:C-HALO` 3).
- **Core point** — a point whose position lies within the partition's own **bounds**,
  rather than one drawn into its halo. Membership is *geometric*, not assigned: the bounds
  state each face as closed or open, and a partitioning assigns closedness so every position
  in its extent lies in exactly one partition — which is what stops a point on the dataset's
  outer face belonging to none. The count is per partitioning, so a point is core of one
  partition per level of detail. Operators read core and halo points
  alike and emit only core points, which is what makes a partitioned run's point count
  equal a whole-dataset run's (`RFC-0002:C-HALO` 1).
- **Halo attribute** — the boolean column marking which points are halo. A **cache** of
  the geometric definition above, not the definition itself: where the two disagree the
  column is wrong. Kept rather than derived at each use because what partition bounds
  cannot recover is the *ordering* — core points first — and the core count that makes
  halo removal a range would otherwise be unfalsifiable (`RFC-0002:C-HALO` 1, 6).
- **Pipeline halo width** — what the *planner* must borrow for a chain of
  neighbourhood operators, as distinct from the width each operator declares for
  itself. Over a single borrowed halo it is the **sum** of the declared widths, not
  the largest: each operator spends a border of validity, so the exact region shrinks
  by one width per operator (`RFC-0002:C-COMPOSITION` 3). Computable before anything
  runs, because `C-EXEC` 1 already requires every width to be declared — which is why
  it is a *planner* obligation: a component seeing one operator at a time could not
  compute it.
- **Halo exchange** — the alternative to borrowing the sum: give each operator its own
  declared width and re-borrow between operators. Exact, and costs a barrier — every
  partition must finish an operator before any starts the next — plus the intermediate
  state that barrier implies. Named for the scientific-computing operation it is. One of
  the two arrangements `RFC-0002:C-COMPOSITION` 2 permits.
- **Declarable summary size** — a summary size bounded by a function of the partition
  size, the halo width and the summary's own resolution, and of nothing about the data.
  For a grid it is the number of cells the partition and its halo **span**, not the number
  occupied: occupancy rises with density, the span does not. A size bounded only by the
  dataset's *extent* is not declarable — extent is a dataset property, and a fully
  occupied grid grows with it. A whole-extent summary therefore has no declarable size and
  must be built per partition (`RFC-0002:C-SUMMARY`).
- **Summary-mediated neighbourhood** — where a later pass reads a gridded summary an
  earlier pass built, rather than reading points. Its halo is still a *point* halo:
  (reach + 1) times the resolution, reducible to reach times the resolution only where every
  partition boundary *coincides* with a summary cell boundary (`RFC-0002:C-HALO` 3). Needs no
  second unit, which is what keeps it commensurable with `C-COMPOSITION` 3's sum.
- **Grid alignment** — every partition boundary coinciding with a summary cell boundary.
  Measured to remove the straddle term entirely, and **not** the same as the resolution
  dividing the partition extent: a grid can divide it exactly and still straddle every
  boundary if its origin is offset. Only coincidence permits the reduced halo
  (`RFC-0002:C-HALO` 3).
- **Dependent chain** — operators where each after the first reads an attribute the
  previous one *derived*. Only a dependent chain needs the composed width; reading only
  pass-through attributes does not compose (`RFC-0002:C-COMPOSITION` 1).
- **Halo removal** — dropping a partition's halo points. Because core points precede
  halo points within a batch, this is a reduction to a *range*: the retained attributes
  keep the same memory rather than being written again (`RFC-0002:C-HALO` 5). Selecting
  points on the halo attribute reaches the same answer by rewriting every attribute,
  which `RFC-0002:C-MATERIALISATION` 1 forbids where the survivors are contiguous.
- **Passed through** / **derived** — an output attribute is *derived* where an operator's
  output for it differs from its input for at least one emitted point; otherwise it is
  *passed through*. Defined by values rather than by declaration so the distinction can
  be checked rather than asserted (`RFC-0002:C-MATERIALISATION` 1).
- **Single materialisation** — point data is written into Arrow arrays once, at decode,
  and every later stage shares those bytes. What makes out-of-core processing *fast*
  rather than merely possible. Note the two different guarantees it rests on: halo
  removal and pass-through are *memory*-preserving (`RFC-0002:C-MATERIALISATION`),
  whereas projection is *value*-preserving (`RFC-0002:C-PROJECTION` 3) — projection is
  about which attributes exist at all, not about who owns their bytes.
- **Disjoint partitioning** — one where every point belongs to exactly one
  partition. Aggregation is correct *only* over a disjoint partitioning; a halo
  partitioning is deliberately not one, and mixing them silently over-counts
  (`RFC-0002:C-HALO` 2).

## Resolution & honesty
- **Level of detail** (*LOD* in code, spelled out in prose) — how much of a
  region's density is requested; the depth in the source's hierarchy. Rendering
  asks for enough for the viewport; processing usually asks for full resolution.
- **Approximate** — output differing from what the same operation would produce
  over the whole dataset at full resolution. Permitted; concealing it is not, and the
  *kind* of divergence must be named, not just its presence (`RFC-0002:C-APPROX`).
- **Exact** — output identical to the whole-dataset formulation. The default
  expectation, and **lost by composition**: an exact aggregate over approximate
  input is approximate.
- **Resolution floor** — the coarsest level of detail an evaluation may draw from.
  One of the two reductions, and declared separately from the other.
- **Extent** — the region an evaluation covers. The *other* reduction. Restricting
  it is exact **only** where every operator declares zero halo and a single pass;
  otherwise it produces a moving artefact at the boundary (`RFC-0003:C-RESOLUTION`
  2).
- **Preview** — an evaluation reduced in either respect. Never authoritative, never
  reusable as output, and not required to be reproducible — but must not be visibly
  unstable for a stationary camera.
- **Committed run** — full resolution over a declared extent, **bit-identical** on
  repetition given the same graph, source fingerprints, and recorded configuration.
  What a delivered result comes from. Export always routes through one.
- **Partition order** — the fixed total order (Morton over node keys) in which a
  committed run combines reductions, *never* completion order. What makes
  bit-identity achievable without a new sequencing concept (`RFC-0003:C-COMMIT` 3).
- **Associative form** — an accumulator whose combination is exactly associative,
  so its result does not depend on the order partials are merged in. Scaled-integer
  accumulation is the usual one. The *second* route to bit-identity, alongside
  partition order, and the only one that survives being handed to a component whose
  scheduler we do not control (`RFC-0003:C-COMMIT` 5).
  Note the two axes: an *accumulator* is associative or is not; an **attribute
  admits** an associative form only if one quantum spans its smallest resolvable
  value and its largest reachable total inside the accumulator's range. The clause
  turns on the second, which is the stricter test.
- **Accumulation quantum** — the step to which an input value is rounded before it
  enters an associative accumulator. Declared per attribute and reported *beside the
  result value*, not only in the run's configuration, so whoever holds the number can
  see how finely it was computed (`RFC-0003:C-COMMIT` 7). Mostly not *chosen*: a source
  storing coordinates as scaled integers has already fixed the finest step it
  distinguishes, so for a passed-through attribute the step is a property of the input and
  may not be declared finer. A stage that derives or requantises declares the step of what
  it produces; nothing leaves it to configuration (`C-COMMIT` 10). Carried as metadata on
  the field, for the same reason the CRS is — with one measured limit: where an expression
  *consumes* a reduction's result the tag does not travel, which is arguably right, since
  twice a total is a different quantity whose precision is not the reduction's. So a quantum
  states something about an **unmodified** result, not blanket provenance.
  Deliberately **not** called *scale*: in fixed-point and decimal representations
  "scale" names a digit count, and reads as a claim that the stored value has been
  multiplied. Nothing in a result is scaled — the quantum is a step size in the
  attribute's own unit. *Quantisation* stays the name of the process.
- **Computed memory bound** — an operator's own upper bound on the memory it will hold for
  a partition, calculated from its configuration plus the partition's bounds, halo and
  point bound, without reading point data. Replaces a declared *figure*, which cannot be
  right across the partition sizes worth using: the two admissible bounds for a gridded
  structure — the region it spans and the points it can hold — were measured more than two
  orders of magnitude apart, so one number fixed in advance sits somewhere in that gap for
  every partition it ever sees (`RFC-0002:C-MEMORY` 1).
- **Point bound** — an upper bound on how many points a region holds, obtained by summing
  the counts a spatial index records for the nodes it intersects. Exact where the region
  aligns to node boundaries and an over-count otherwise, so it is a *bound* and not an
  estimate — which is what keeps a computed cost a ceiling. Its tightness depends on how
  deep into the hierarchy the planner reads, so the level used is recorded with it
  (`RFC-0002:C-MEMORY` 2).
- **Pipeline peak** — for one partition and one order of the operators, the greatest total
  over any step of the bounds counting at that step. Liveness comes from the *declaration*
  `RFC-0002:C-EXEC` 1 requires — which structures outlive an operator and which later
  operator needs them — never from a claim about timing, because a timing claim is not
  checkable. Not the sum over all operators, which charges a pipeline for phases it never
  holds at once; measured, the sum exceeded the peak by 1.50 to 1.61 times
  (`RFC-0002:C-MEMORY` 3).
- **Invariance to partitioning** — the property that lets a planner size partitions to the
  machine rather than to a constant. A sufficient halo makes a partitioned neighbourhood
  computation agree with its whole-dataset equivalent, and an associative accumulation gives
  the same total however partials were grouped, so partition size does not reach the result.
  Established against the input in a *single* partition, never by comparing two chosen
  partitionings — two badly chosen ones can agree with each other
  (`RFC-0003:C-PARTITIONING` 1).
- **Adaptive partitioning** — the cache reshaping partitions to match how a user is
  working: merging nodes while the camera orbits one tree, subdividing them for a finer
  working set. Permitted for preview and rendering, forbidden for a committed run — but not
  because it would change the answer. A partitioning is free to vary where every operator is
  invariant to it, and most are. It is forbidden because a partitioning that followed a
  camera cannot be *accounted for*: it is not expressible as a recorded partition size, so
  the run cannot be replayed or defended (`RFC-0003:C-PARTITIONING` 1, 2). Never *subdivide*
  to get neighbours — that is the planner's job from the declared halo width
  (`RFC-0003:C-PARTITIONING` 3).
- **Projection** — the set of attributes a pipeline actually reads, computed from
  operators' declarations and pushed into the source. Free, since `C-EXEC` 1 already
  requires the declaration. Must not change results — which makes it a *test* for
  operators reading attributes they never declared. Gain over LAZ is bounded to
  materialisation, not I/O: a compressed chunk is a unit, so the full columnar win
  needs a columnar format.

## Editing
- **Document graph** — the authoritative structure describing what the data *is*:
  the source, the operators applied to it, and the edits. Everything a user sees is
  derived from it; nothing else is authoritative (`RFC-0007:C-EXTRACT` 1). Borrowed
  in spirit from Graphite, where the document *is* the graph.
- **Edit** — a user correction, stored as **the gesture, not the points**. A lasso
  over 40 000 points is a polygon, a predicate and an action — about a kilobyte,
  whatever it touched.
- **Region edit** — the default form: a spatial region + optional attribute
  predicate + action. Needs no point identity, and composes with the octree index
  because it *is* a spatial predicate.
- **Point-set edit** — the escape hatch: an explicit enumeration, size-capped, for
  corrections no region expresses (the twelve stray returns on a wire). Requires
  point identity, and is deliberately not the default (`RFC-0007:C-EDIT` 3).
- **Edit stack** — the ordered sequence of edits. **Ordered, not a set**: reordering
  changes the result unless independence can be shown.
- **Effective edit set** — the edits that apply to one partition, found by spatial
  query over the stack's index rather than by replaying the stack.
- **Layer stack** — the user-facing projection of the document graph. Users see
  layers and a history list; the graph is underneath. Graphite's transferable
  lesson — foresters are not node-graph people.

## Render synchronisation
- **Render state** — the derived, GPU-side representation the renderer draws from.
  Never authoritative, and never written back to the document (`RFC-0007:C-EXTRACT`
  2). Bevy's *render world*.
- **Extract** — the single synchronisation point where document and render state are
  both accessed. **Metadata only** — which partitions are visible, which edits apply
  — never point bytes and never I/O, because it is the one place nothing may block.
  Bevy's *Extract*, with the deliberate difference that it copies *plans* rather
  than values, since the data is not resident.
- **Retained** — describes render state that persists across frames and is
  invalidated incrementally. Contrast a general-purpose engine, which clears render
  state each frame; here that would mean re-uploading a viewport of points per
  frame, the exact cost the project exists to avoid.
- **Bake** — transparent consolidation of the edit stack's *executable*
  representation when a region's chain gets expensive. A cache: re-derivable from
  history, discardable at any time, and forbidden from changing results.
- **History** — the append-only record of every edit, including undone and
  superseded ones. Never collapsed by baking or eviction. Distinct from *undo
  state*: undo needs the previous state, provenance needs the record that a rejected
  state existed.

## Identity
- **Point reference** — how an individual point is named: `(source, version
  fingerprint, node key, index within node)`. A **position in a versioned
  container**, not a minted id. Free, needs no sidecar, and is what both QGIS
  (`sub-index, node id, index`) and CloudCompare (array index) use.
- **Version fingerprint** — a streaming BLAKE3 digest binding a reference to the
  exact source bytes it was taken against: header + index hierarchy where the format
  has one, whole file where it doesn't (LAS). Not a security property — a content
  digest, and a persisted compatibility surface (`RFC-0005:C-BINDING`).
- **Stale reference** — a reference whose fingerprint no longer matches its source.
  Never migrated, never re-resolved by proximity; the document opens **read-only**
  and names the affected edits.
- **Source substitution** — replacing a source in place with a reprocessed file.
  Every node key and index still resolves and now means *different points*. The
  hazard non-destructive editing creates and destructive editors cannot have, since
  for them the file *is* the state.

## Presentation
- **Host surface** — the opaque native drawing target the renderer is *given*. The
  renderer creates no windows, owns no thread, and never learns which toolkit
  produced it — the same arrangement `C-HOST` applies to storage, extended to
  presentation (`RFC-0006:C-SURFACE`).
- **Direct presentation** — the renderer presents to the surface itself. Route taken
  first: zero graphics interop, and a browser canvas yields the same kind of handle.
- **Offscreen compositing** — the renderer draws into a target the *host* composites
  into its scene graph. Admitted by the same contract from the start, adopted when
  depth-insensitive overlay is wanted. Needs per-backend texture interop, which is
  why it is second.
- **Depth-dependent content** — anything whose correctness relies on depth against
  the cloud: measurements anchored to points, in-scene selection geometry. **Must be
  renderer-drawn** — composited content carries no depth and floats in front of
  geometry it should be behind (`RFC-0006:C-OVERLAY` 1).
- **Depth-insensitive content** — interface chrome with no spatial relationship to
  the scene. May be composited by the host. The only thing overlay actually buys.
- **Toolkit confinement** — Qt exists only in the application crate. Driven by
  `C-LICENSE` 4 (Qt is LGPL-3/GPL-3), and holds even against a commercial Qt licence,
  which would remove the obligation for *this* project while leaving every adopter
  bound (`RFC-0006:C-TOOLKIT` 2).

## Coordinates
- **CRS** — a coordinate reference system, carried by every source and stage and
  **opaque** to library crates: comparable for identity, never parsed. Interpreting
  one needs a transformation database a library crate may not open
  (`RFC-0005:C-CRS` 2). Carried as metadata on the coordinate **fields**, not on the
  batch: a batch has one slot, and an operation combining two systems keeps one of them
  without saying which. An operation combining coordinates whose systems are not
  identical is refused (`RFC-0005:C-CRS` 1, 4).
- **Borrowed encoding** — Strider spells a CRS in GeoArrow's vocabulary (`crs`,
  `crs_type`, `edges`) and takes those definitions from the published crate rather than
  restating them, while keeping the *enforcement* the specification leaves unspecified:
  it says how to write a system down, not what a consumer does when two disagree.
  Adopting an encoding is not adopting a safety property (`RFC-0005:C-CRS` 5).
  Coordinates stay **sibling** fields inside a pipeline so one axis can be projected
  alone, and are composed into the conformant single-field form only where they leave
  Strider — a composition that moves no bytes.
- **Vertical reference** — the surface heights are measured from. Ellipsoidal and
  orthometric differ by *tens of metres*. Mismatched or absent → any
  height-above-a-surface operation **fails** (`RFC-0005:C-VERTICAL`). Absence is
  never treated as agreement.
- **Transform capability** — host-supplied coordinate transformation, injected like
  storage and retrieval. PROJ is MIT so licensing permits it, but it links SQLite and
  reads a database from disk, so it lives in the application only.
- **Query-side transformation** — the default and the cheap one: transform the *query
  bounds* into the source's CRS, leave the points alone. Transforming points is
  per-point trigonometry over billions, and requantises the scaled integers that make
  exact accumulation and stable references possible.
- **Reference backend** — the designated authoritative transformation
  implementation. A committed run involving a **datum shift** must use it; other
  transformations may use any backend but must record which one, at what version,
  with which grid database (`RFC-0005:C-BACKEND`).
- **Datum shift** vs **projection** — a projection is closed-form, so correct
  implementations agree to float precision. A datum shift depends on grid files and
  pipeline selection, so correct implementations can differ by decimetres. Hence one
  rule for each, not one rule for both.

## Extension
- **Operator origin** — where a registered operator came from, and whether its
  conformance was **verified by someone other than its author**. Recorded per
  committed run, and unverified output is surfaced to the user the same way
  approximate output is (`RFC-0002:C-ORIGIN`).
- **Expression** — user-supplied logic that is a *pure function of one point's
  attributes*: no I/O, no state, no other points. **Safe by construction** — every
  contract it could break is unreachable from it, not merely checked. Must be
  evaluable vectorised over Arrow arrays; per-row interpretation is unusable at
  billions of points.
- **Serialised document graph** — the pipeline description. Not a separate format:
  `C-PROVENANCE` 2 already requires the graph to be recorded, so the record *is* the
  pipeline (PDAL's arrangement, at no cost).
- **No plugin mechanism** — an operator is written in-tree or by a consumer building
  against the published crates. Runtime-loaded native libraries are excluded: they
  have ambient access (breaking `C-HOST`), don't exist on constrained targets
  (breaking `C-PORT-GATE`), and a fault takes the session with it. Sandboxed WASM
  operators are the intended eventual answer, with conditions already stated.

## Releases
- **Committed vs claimed** — a release asserts a property only once its verification
  passes. Bounded memory, halo correctness, and bit-identical repetition are the three
  claims that fail *invisibly*, so each is a gate (`RFC-0001:C-CONFORMANCE`).
- **Halo verification** — partitioned output compared against output computed with the
  input in a **single partition**. For a non-approximate operator these must be
  *identical*: a sufficient halo makes partitioning exactly lossless, so any
  difference means the width is under-declared. Not a tolerance. The one test that
  cannot be faked.
- **Refusal half / capability half** — every honesty obligation has both. Refusing a
  mismatched CRS or vertical datum is conformant *on its own*, so a release can be
  honest without being complete: "transformation is not implemented" is a position,
  not a gap.

## Caching
- **Four caches, not one** — byte-range (fingerprint + offset), node/decoded
  (fingerprint + node + attribute), bake overlay (fingerprint + edit-stack prefix),
  and render state (partition + effective-edit-set version). Different keys,
  lifetimes and consequences; conflating them would give the least trustworthy the
  reach of the most.
- **Cache as decorator** — a byte cache *is* an implementation of the retrieval
  interface wrapping another, not a layer beside it. Because retrieval is explicit
  offset+length with multi-range batching, coalescing and dedup are portable logic
  while the backing store stays host-supplied (`RFC-0004:C-CACHE` 3).
- **Per-attribute keying** — decoded entries are keyed per attribute, never per
  requested projection. Otherwise `C-PROJECTION` forces a choice between fragmenting
  the cache per projection and caching whole records, which throws away the point of
  projection.
- **Not a VFS** — deliberately. Paths, handles, seek and permissions are *more*
  privilege than caching needs. A keyed store with no paths has no path traversal; a
  write-once store has no TOCTOU. The narrowness **is** the security property.
- **Entry integrity** — every entry carries a digest of its own content. Verified
  where it feeds a **committed run**, optional for preview. Failure means
  *discard and re-derive*, never an error — always possible, since no cache is
  authoritative. Detection, not prevention: an altered entry may not silently reach a
  delivered result (`RFC-0004:C-INTEGRITY`).

## Host & platform
- **Host** — whoever supplies a library crate its environment capabilities. The
  desktop application is one host; a server or a browser page would be others.
  Library crates never reach for capabilities ambiently (`RFC-0004:C-HOST`).
- **Storage interface** — host-supplied persistent byte storage for spill, cache,
  and working sets. Keyed, not hierarchical; write-once, not modify-in-place —
  shaped by the weakest plausible backend rather than by a filesystem.
- **Retrieval interface** — host-supplied byte-range reads, expressed as explicit
  offset and length (never a seekable handle) and batchable into one multi-range
  request.
- **Spill** — moving part of a working set to the storage interface to stay within
  the memory ceiling. A permitted strategy, not a fallback from failure.

## Unresolved

Nothing outstanding from the architecture grill — all fourteen questions in
`.govctl/grill/strider-architecture/state.toml` are resolved. New terms land here
first when the next design question opens one.

