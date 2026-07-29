# Naming Diagnosis: Streaming Point Cloud Workbench

**Date:** 2026-07-29
**Project:** Rust + Qt6 out-of-core streaming point cloud processing library & desktop application
**State:** ✅ Decided — **Strider**

---

## 1. Project Essence

Before evaluating names, we need to crystallize what this thing _is_:

| Dimension | Description |
| ----------- | ------------- |
| **Core innovation** | Out-of-core, streaming point cloud processing — you never load the whole cloud into RAM |
| **Architecture** | Arrow-native internal representation; COPC/GeoParquet/E57/LAS as source adapters; composable pipeline/DAG of streaming transforms |
| **Key differentiator** | CloudCompare's processing power + QGIS's streaming model — this combination doesn't exist in FLOSS today |
| **Primary domain** | Forestry LiDAR, but general-purpose |
| **Tech stack** | Rust (library + engine) + Qt6 (desktop GUI) |
| **Scale target** | Terabyte-scale point clouds — billions of points |
| **Output** | A library crate ecosystem + a desktop application |

The ChatGPT conversation distilled this architectural insight:

> _"Rather than another monolithic GIS, build an Arrow-native, streaming point cloud workbench where visualization and processing both operate on lazily loaded spatial tiles."_

---

## 2. Naming State: Pre-Diagnosis

This is a greenfield naming situation. No candidates exist to fix. Instead, we apply the four-layer framework to _generate and evaluate_ candidates.

---

## 3. Layer Requirements

### Sound Layer

The name should blend:

| Sound category | Examples | Why |
| --------------- | ---------- | ----- |
| **Flow sounds** (l, r, w) | l, r, w, vowels | Communicates streaming, movement, continuity |
| **Power sounds** (k, t, p) | k, t, p, d, str- | Communicates performance, handles massive data |
| **Depth sounds** (o, u, m) | o, u, m, n | Communicates gravitas, serious engineering |

Avoid:

- Harsh-only sounds (too aggressive for a tool)
- Weak-only sounds (doesn't communicate performance)
- Overly exotic phonemes (accessibility matters)

### Meaning Layer

Ideal: a name that works on _multiple_ levels:

- **Literal:** References points, clouds, streaming, or spatial data
- **Metaphorical:** References flow, movement, traversal, crafting
- **Cultural:** Feels at home in the Rust ecosystem + geospatial domain

### Cultural Layer

| Context | Convention | Implication |
| --------- | ----------- | ------------- |
| Rust crates | Short, lowercase, one word; often creative (`serde`, `rayon`, `pasture`, `bevy`) | Crate name should be ≤2 syllables, distinctive |
| Geospatial tools | Often acronyms (PDAL, QGIS, COPC, SMRF) or descriptive (CloudCompare, Potree) | Acronym not required; descriptive or metaphorical both work |
| Desktop apps | Capitalized, memorable, brand-able | App name can be more expressive than crate name |

### Functional Layer

| Requirement | Criterion |
| ------------- | ----------- |
| crates.io availability | Name (or namespaced variant) must be available |
| GitHub org/name | Should be available |
| Domain | Nice to have, not critical for FOSS |
| Searchability | Distinctive enough that search returns this project |
| Pronunciation | Intuitive from spelling |
| Typing | No awkward key combinations |

---

## 4. Naming Territories

### Territory A: Flow / Stream Metaphors (streaming is the core innovation)

**Rationale:** The architecture is fundamentally about data flowing through a pipeline without being fully resident in memory.

- **PointFlow** — points + streaming
- **Rill** — small stream (water metaphor)
- **Fluence** — flowing + influence
- **Torrent** — powerful flow (but piracy association + crate conflict)
- **Flume** — artificial water channel = pipeline (but major existing crate)

### Territory B: Movement / Traversal (out-of-core = striding over data)

**Rationale:** Out-of-core means you never "sink into" the data — you stride over it.

- **Strider** — one who strides; strides over massive data without sinking into RAM
- **CloudStrider** — striding over point clouds
- **Drift** — movement with flow

### Territory C: Spatial / Terrain (point clouds + forestry)

**Rationale:** The domain is spatial data, terrain, and forestry.

- **Overstory** — forest canopy layer; beautifully forestry-specific
- **Crest** — peak, ridge
- **Swale** — terrain depression (hydrology/GIS relevant)
- **Arroyo** — dry creek bed (southwest terrain feature)

### Territory D: Technical / Explicit (directly names the differentiator)

**Rationale:** The Rust ecosystem values clarity. Just say what it does.

- **OutCore** — out-of-core processing
- **StreamCore** — streaming core

### Territory E: Craft / Shaping (processing = crafting raw data)

**Rationale:** The tool processes/shapes raw point cloud data into useful products.

- **Hew** — to shape by cutting; forestry tie-in (hew timber)
- **PointMill** — milling/processing points
- **PointForge** — forging points (but generic)

---

## 5. Candidate Evaluation

### Tier 1 (Strongest)

#### 5.1 Strider

| Layer | Assessment |
| ------- | ----------- |
| **Sound** | ★★★★☆ — "Stri-der": power cluster (str-) + flow vowel (i) + depth consonant (d, r). Energetic but smooth. Two syllables. |
| **Meaning** | ★★★★★ — Core metaphor: striding over terrain without sinking in = out-of-core processing. "The data is too big to hold, so Strider walks over it." Active, forward-moving. |
| **Cultural** | ★★★★☆ — Fits Rust naming conventions (compare: `stride`, `rstar`). Distinctive in geospatial. No acronym clash. |
| **Functional** | ★★★☆☆ — `strider` crate exists but **abandoned since 2016** (v0.1.3, ringbuffer ops). Name transfer likely achievable from owner `snd`. Alternatively: `strider-pointcloud` / `strider-rs`. GitHub: `strider-pointcloud` likely available. |

**Library ecosystem:** `strider-core`, `strider-io`, `strider-algo`, `strider-view`
**Application name:** "Strider" or "Strider Studio"
**Tagline potential:** "Stride over your data."

**Verdict:** The strongest metaphor of any candidate. The "striding over" image captures both out-of-core (never sinking into RAM) and streaming (continuous movement). The abandoned crate is a solvable problem. Worth the effort.

---

#### 5.2 PointFlow

| Layer | Assessment |
| ------- | ----------- |
| **Sound** | ★★★☆☆ — "Point-Flow": power consonant (p, t) + flow (l, w). Two balanced syllables. Solid but not remarkable. |
| **Meaning** | ★★★★☆ — Points + streaming/pipeline. Clear, immediate, technically precise. Works on both literal and metaphorical levels. |
| **Cultural** | ★★★★☆ — Fits Rust conventions. Fits geospatial (many "point*" tools). Not an acronym — stands out from QGIS/PDAL pattern. |
| **Functional** | ★★★★★ — **Available on crates.io.** `pointflow` is used by a JS/React library (npm) but not registered as a Rust crate. Clean namespace: `pointflow-core`, `pointflow-io`, etc. |

**Library ecosystem:** `pointflow-core`, `pointflow-io`, `pointflow-algo`, `pointflow-view`
**Application name:** "PointFlow" or "PointFlow Studio"
**Crate name:** `pointflow`

**Verdict:** The safest, clearest choice. Immediately available. What it loses in distinctiveness it gains in clarity. If Strider's crate transfer proves difficult, this is the strong fallback.

---

#### 5.3 OutCore

| Layer | Assessment |
| ------- | ----------- |
| **Sound** | ★★★☆☆ — "Out-Core": power consonants (t, k, r). Two syllables. Technical, direct. |
| **Meaning** | ★★★★☆ — Names the key technical differentiator: out-of-core processing. Immediately communicates to technical audiences. |
| **Cultural** | ★★★★☆ — Very Rust-like (short, lowercase, technical). In-group knowledge signals. |
| **Functional** | ★★★★☆ — **Available on crates.io.** Clean. But `outcore-core` is awkward repetition if that crate name is needed. |

**Library ecosystem:** `outcore-io`, `outcore-algo`, `outcore-view` (but `outcore-core` is awkward)
**Application name:** "OutCore" — works, but sounds jargony for an app

**Verdict:** Excellent for the library; awkward for the app. Best if paired with a different application name, but that fragments the brand.

---

### Tier 2 (Good with caveats)

#### 5.4 Overstory

| Layer | Assessment |
| ------- | ----------- |
| **Sound** | ★★★★☆ — "O-ver-sto-ry": depth sounds (o, o), flow sounds (r, y). Three syllables, elegant. |
| **Meaning** | ★★★★☆ — Forest canopy layer. Perfect for the forestry primary use case. Suggests "overview" / "above it all" — viewing from above. |
| **Cultural** | ★★★☆☆ — Distinctive. But an npm package `@os-eco/overstory-cli` exists (AI orchestration, archived). No Rust crate conflict. |
| **Functional** | ★★★★☆ — Available on crates.io. GitHub: may need `overstory-pointcloud`. |

**Concern:** Pins the project to forestry. If point cloud processing expands to urban planning, bathymetry, industrial scanning, etc., "Overstory" becomes misleading.

**Verdict:** Beautiful if forestry is the permanent focus. Risky if the project will generalize.

---

#### 5.5 Hew

| Layer | Assessment |
| ------- | ----------- |
| **Sound** | ★★★☆☆ — "Hyoo": one syllable, breathy. Minimalist but weak. Easy to miss in conversation. |
| **Meaning** | ★★★★★ — To shape by cutting (wood, stone). Forestry tie-in: hewing timber. Processing metaphor: shaping raw data. |
| **Cultural** | ★★★☆☆ — Ultra-minimal. `hewdiff` crate exists (diff viewer, binary called `hew`). `hewn` crate exists (game engine). Crowded space. |
| **Functional** | ★★☆☆☆ — `hew` itself may be available as a crate name, but `hewdiff` and `hewn` create confusion risk. Voice recognition: easily misheard as "hue" or "hugh." |

**Verdict:** Gorgeous meaning, weak practicals. Too short to be distinctive; too much name-space crowding.

---

#### 5.6 Rill

| Layer | Assessment |
| ------- | ----------- |
| **Sound** | ★★☆☆☆ — One syllable, all soft sounds. Flows well but lacks power. |
| **Meaning** | ★★★☆☆ — A small stream. Nice flow metaphor, but "small" is wrong for terabyte-scale data. |
| **Cultural** | ★★★★☆ — Distinctive in software. Short like `rill`. |
| **Functional** | ★★★★☆ — Likely available on crates.io. But easily misspelled (rill vs ril vs rille). |

**Verdict:** Promising but the "small stream" connotation undermines the "massive data" message.

---

### Rejected

| Name | Reason |
| ------ | -------- |
| **Torrent** | Piracy association + active BitTorrent crate |
| **Flume** | Major existing crate (182M downloads, MPSC channel) — intractable confusion |
| **Stratus** | Multiple active crates; cloud type name overused in tech |
| **Spate** | Active ETL pipeline crate — direct conceptual overlap with this project |
| **Quiver** | Taken by Rerun.io for Arrow column wrappers — too close conceptually |
| **Fletcher** | Checksum algorithm crate — unrelated but well-established |
| **Cascade** | Heavily used across crates.io |
| **Drift** | Negative connotation (aimless, drifting) |
| **Crest** | Passive imagery (a crest is still, not flowing) |

---

## 6. Recommendation

### Primary: **Strider**

```
Crate ecosystem:  strider-core | strider-io | strider-algo | strider-view
Desktop app:      Strider
Tagline:          "Stride over your data."
```

**Why it wins:**

1. **Metaphor precision.** "Striding over terrain" perfectly captures out-of-core processing — you move across the data without ever sinking into it. No other candidate has a metaphor this apt.

2. **Sound-meaning alignment.** The `str-` onset (strong, forward-moving) + the `-ider` coda (smooth, continuous) mirror the architecture: powerful processing, streaming flow.

3. **Distinctiveness.** No other geospatial tool uses this metaphor. It stands out from acronyms (PDAL, QGIS) and descriptives (CloudCompare, PointFlow).

4. **Ecosystem fit.** `strider-*` crate namespace is clean and natural. Fits Rust conventions.

5. **Growth room.** The name doesn't pin you to one domain (forestry), one format (COPC), or one pattern (just visualization). It scales with the architecture.

**Obstacle:** `strider` crate is taken (abandoned, 2016). Mitigation:

- Request name transfer from `snd` (common on crates.io for abandoned crates)
- Fallback: publish as `strider-pointcloud` with `strider-core`, `strider-io`, etc. as sibling crates
- The application name "Strider" is unaffected by crate naming

### Fallback: **PointFlow**

```
Crate ecosystem:  pointflow-core | pointflow-io | pointflow-algo | pointflow-view
Desktop app:      PointFlow
```

**Why it's the fallback:**

- Available immediately on crates.io with zero friction
- Clear, descriptive, technically accurate
- Less distinctive but lower risk

---

## 7. Name System Design

Regardless of which name is chosen, the multi-crate ecosystem should follow this pattern:

```
{name}-core        — Core traits: PointBlock, SpatialSource, PointAlgorithm
{name}-io          — Format adapters: COPC, LAS/LAZ, GeoParquet, E57
{name}-algo        — Processing algorithms: ground classification, normals, CSF, voxelization
{name}-view        — Rendering engine (wgpu-based), LOD management, node cache
{name}-app         — Qt6 desktop application (or just the app name for the binary)
```

This separation maps directly to the architecture in the ChatGPT conversation:

```
SpatialSource  →  Stream<RecordBatch>  →  PointAlgorithm  →  Stream<RecordBatch>  →  Renderer
    ↑                                         ↑                                        ↑
{name}-io                               {name}-algo                              {name}-view
```

---

## 8. Decision

**Chosen: Strider**

Rationale: The "striding over terrain without sinking in" metaphor captures out-of-core processing with unmatched precision. Distinctive in the geospatial ecosystem. Clean `strider-*` crate namespace.

## 9. Next Steps

- [ ] Check GitHub: `strider-pointcloud` org or repo availability
- [ ] Request `strider` crate name transfer from `snd` (abandoned 2016) or register `strider-pointcloud`
- [ ] Register `strider-core`, `strider-io`, `strider-algo`, `strider-view` if going umbrella-crate route
- [ ] Verify domain/social handles (nice to have, not blocking)
- [ ] Proceed to `/grill-gov` for RFC/ADR governance scaffolding

---

_This analysis follows the naming skill's four-layer diagnostic framework. The recommendation prioritizes metaphor precision and distinctiveness (Strider) with a low-friction fallback (PointFlow). Decision made 2026-07-29._
