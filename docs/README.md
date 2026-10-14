# Octavis (internal notes)

Standalone native desktop 3D modelling tool for Minecraft builds (schematic scale, not infinite worlds). "MCEdit upgraded, with Blender-style UX." No running Minecraft needed. Keep this file short and current; public README comes at MVP.

Priorities: performance, stability, extensibility.
Platform: Windows / macOS / Linux. Remote: https://github.com/21krstic/Octavis (empty, push verified via dry-run).

## Stack

Rust, winit, wgpu, egui + egui_dock, glam, fastnbt, image, serde, rayon. Noise is hand-rolled (`octavis-gen/noise.rs`): no libm calls and literal gradient tables so a seed gives identical results on every platform (golden-value test guards this).
Later: mlua (Lua generators/brushes), egui node-graph crate (generator graphs).

## Architecture

```
[Editor UI (egui)] -> [Core Voxel Engine] <-> [I/O layer: NBT, schematics, OBJ, packs]
                            |                  [Generators: noise, terrain, Lua]
                      [Renderer (wgpu)]
```

- **Rule:** the core engine depends on nothing else (no rendering, UI, or MC version). Format-specific code lives only in I/O. (MCEdit died from format coupling at 1.13.)
- Grid: **growable on all 3 axes**, chunked into 16^3 sections, palette-compressed with bit-packed indices (mirrors MC's format so round-trips are lossless). Add a size cap only if needed.
- Cell = namespaced block state (`minecraft:oak_stairs[facing=north]`) + optional block-entity NBT.
- Layers (`octavis-core/layers.rs`): each layer owns a sparse `Grid`; topmost shown non-air wins. The editor keeps a derived *composite* `World` (what tools, meshing, picking and export read) updated per write by `LayerStack::write`, and rebuilt on structural changes. `World` = block table + `Grid` (derefs to it); layers share the one table.
- Undo: command pattern, one sparse diff per action/stroke.
- **Block registry** (version-independent: properties, shapes, connectivity rules) is a core dependency, not an I/O detail. Needed by fix-connect and type replacer.
- Not an octree (target format is MC's chunk+palette) and not BMesh-based; borrow Blender's UX only.

## Open design decisions (settle before building the relevant milestone)

- **Layers vs. generators: decided.** A layer is `Painted` (tools edit it) or `Generator` (cells are the cached result; read-only to tools; the recipe lives in `OctavisApp::recipes` keyed by `LayerId`, with no payload in core). Regenerating replaces exactly that layer's cells (`replace_cells`), so layers below are untouched. Baking turns a generator layer into a painted one.
- **Undo memory (before generators/large ops):** storing old+new values per changed cell is fine for strokes, but a generator over a huge region could create a gigantic undo entry. Plan: cap total undo memory; when exceeded, spill older entries to disk or drop the oldest. Generator layers can undo by re-running, so they need no cell diff.
- **Rendering:** greedy meshing conflicts with per-face AO/lighting and varied textures. Prefer culled meshing + AO; greedy only for uniform flat regions.

## Textures / licensing

Never bundle Mojang assets. Texture packs are loaded as a **plain folder** (not zipped, not a jar); the user extracts their own pack/jar assets into one. Dev/testing uses the owner's default pack. Eventual goal: ship a placeholder/open texture pack so the app works without owning Minecraft.
Pass 1: full cubes via atlas/texture array. Pass 2: blockstate+model JSON (stairs, fences, slabs), the hardest subsystem.

## Features

**Core:** layers, selection (cuboid/sphere/wand/lasso, boolean ops), brushes (shape = WHERE, pattern/mask = HOW; smooth/erode/blend/splatter/overlay/replace), composable mask/pattern expression language (from day one), procedural 3D density-field generation, Lua scripting, node-graph generator editor (stretch), resource-pack rendering, image/OBJ -> blocks (LAB colour match, dithering, voxelize + fill).

**IO:** import and export from the same milestone as `.schem` (Sponge), `.nbt` (structure block), possibly `.litematic`; export `.obj`+`.mtl` with baked atlas. No raw region/anvil unless scope grows.

**Accepted extras (priority roughly in order, later = lower):**
reference images/ghost overlay, symmetry + array tools (live preview), stamps/prefab library, palette tools (gradients, usage stats), heightmap/image terrain import, redstone/state awareness (show powered/open etc.), measurement tools, project history (versioned, diffable), material list (low priority), collaboration (long-term, not soon).

**Rejected for now:** light/spawn preview, screenshot/render/camera paths, extra plugin systems (WASM), entity support.

**Steal list:** VoxelSniper brush/performer split + blend/erode; FAWE masks/patterns as expressions + streamed placement; Arceon type replacer, fix-connect, arch/road/loft/roof/boulder generators, Voronoi/fractal/proximity/angle/Y masks; ezEdits gradient + spline tools; Axiom camera/docking/boolean selection/previews.

**Wedge vs Axiom:** standalone (zero mod conflicts), headless CLI batch jobs, external import/export, node-graph generators, non-destructive layers, version-decoupled formats, works without MC.

## Dev notes

- Crates: `octavis-core` (voxels, no deps but glam), `octavis-mesh` (CPU meshing: culled + per-vertex AO), `octavis-io` (file formats; `.schem` so far), `octavis-gen` (procedural generators; terrain so far), `octavis-app` (eframe + wgpu viewport; file dialogs via rfd).
- Windows: run cargo from **PowerShell**, not Git Bash (Git's `link` shadows MSVC `link.exe`). Needs MSVC Build Tools.
- Vulkan crashes at startup on the dev machine; the app defaults to DX12 on Windows (`WGPU_BACKEND` overrides).
- Run: `cargo run --release`; test: `cargo test --workspace` (both from PowerShell, repo root).
- First full build is slow (~13 min, deps compile at opt-level 3); incremental is seconds.
- Palette in the shader is a 2048-entry placeholder (ids 0-2039 real, 2040-2047 overlays; ids past that show magenta), replaced by texture packs at milestone 7.

## Status

Milestones 1-5 done (QA-accepted; remaining items are in the backlog below). MVP reached; public README lives at the repo root (keep it in sync with features).

1. Core grid, mesher, rendering, orbit camera (zoom-scaled orbit/pan speed).
2. Pencil (Free stroke mode default; Plane-locked kept as an option), select box / sphere / wand (wand ignores air, optional diagonals), undoable selection changes, axis gizmo + origin axes, selection caps (500k cells; sphere radius 49; wand 200k).
3. Brush tool: sphere/cube/cylinder shapes; paint/replace, splatter, overlay, smooth, erode, dilate, erase. Mask language (`air solid exposed surface selected y>=5 <block> ! & | ( )`) and weighted patterns (`70%stone,30%dirt`) live in core `brush.rs`.
   - Strokes dab along the traced cursor path (no gaps) and pick their target against the world as it was when the stroke began (`Editor::was_solid`), so they never dig or build under themselves. Seed is fixed per stroke, so splatter density is the stroke's coverage.
   - Smooth/erode/dilate run on a dense copy of the area (`Grid`) with a Passes setting (like WorldEdit's smooth brush iterations).
   - `selected` mask term restricts a brush to the selection; "Apply to selection" runs the brush over every selected cell.
   - Not yet: stamp shape (waits for stamps/prefabs), falloff, block properties in patterns/masks (`oak_stairs[facing=north]`), noise/Y-gradient patterns.

4. `.schem` (Sponge) import + export, File menu (`app/files.rs`, format code in `octavis-io/src/schem.rs`).
   - Reads Sponge v1-3 (gzip or raw NBT); writes v2 (widest compatibility), root tag named `Schematic`, DataVersion 3955 (`DEFAULT_DATA_VERSION`).
   - New scene / Open (replaces scene, frames camera, asks before discarding edits) / Import (pastes at camera target on the highest block there, selects it, one undo step; schematic air does not overwrite) / Export (selection if any, else all non-air blocks).
   - Not supported yet because core has no storage for them: block entities (chest contents, signs) and entities. They are dropped on import with a status-bar warning; export writes none. Schematic `Offset` is ignored.
   - Verified against real files from external sites. Limit: bounding box of 64M cells (`MAX_CELLS`); bigger files give a clear error. Supporting them would need streaming the block data straight into the world (the reader currently holds the NBT, an index array and a placement list).
   - Not done: `.nbt` (structure block), `.litematic`, move/transform of pasted selections.

5. Built, in QA: noise terrain generator (`octavis-gen`: `Perlin2` + fBm, `Terrain::plan`; panel in `app/generate.rs`). Heightfield from fractal noise; three patterns for surface / soil / deep layers. Area is either a manual square (centre X/Z, size, depth; independent of the camera, with a "centre on camera target" button) or the selection (its bounds, and only its selected cells, so non-box selections work). One undo step, deterministic per seed, cap 4M cells per run.
   - **Live preview** (`TerrainPreview`, Enter applies, Esc cancels; low-quality shell mode on by default via `Detail::Shell`: only the exposed skin is written, ~14x fewer changes; Apply always generates the full terrain): writes the terrain into the world untracked (`Editor::apply_untracked`) keeping a backup, recomputes (throttled to ~150 ms) when settings change, and Apply commits it as one undo step (`commit_applied`) / Cancel restores. Editing tools, undo, file actions are locked or cancel it while active.
   - Preview idea parked: a non-voxel overlay mesh (heightfield) would be instant and need no tool locking, but must hide the old world beneath it and needs a new render path. Revisit with layers, where a generator layer needs its own non-destructive preview anyway.
   - Not yet: non-destructive regeneration (waits for layers: a generator layer will store these params + seed), 3D density / caves, biomes, water, trees, edge blending with existing terrain.

6. Built, in QA: layers (`app/layers.rs`, core `layers.rs`): add / delete / reorder / merge down, visibility, lock, solo, rename; tools edit the active layer; import creates a layer; terrain lives in generator layers (panel edits the active generator layer's recipe; "New terrain layer" creates one). Undoable: cell edits, add, delete, move, merge (structure commands in history hold removed layers by move, no copies). Not undoable (view state): visibility, solo, lock, rename, active layer, bake. Export uses the composite, so hidden layers are not exported.
   - Known: composite duplicates cell memory (layers + composite); visibility toggles rebuild the whole composite; "Delete blocks" / fill only touch the active layer; no per-layer opacity or blend modes; recipes are not saved anywhere yet (no project file).

Default scene: flat 480x480 grass/dirt/stone platform; grey viewport background.

## Known issues / backlog (from QA)

- **Performance on weak hardware** (dev laptop is weak): radius-16 smooth and other large operations spike the CPU. Ideas: parallelise planning with rayon, mesh dirty sections off the main thread, faster section hashing, incremental overlay meshing.
- **Erode makes 45/90-degree patterns when repeated**: expected from the rule (it removes corners, so edges chamfer into planes). Possible later fix: randomness or a threshold option.
- **Box select** needs a real fix: corners depend on hovering existing blocks (open-air fallback is only the anchor's horizontal plane). Needs a proper way to define two points in space (e.g. drag a base rectangle, then extrude height).
- **Large sphere selections lag**: the selection overlay is rebuilt (temp world + meshing) every frame of a drag. Fix by incremental/cached overlay meshing or a cheaper overlay for big selections.
- **Pencil**: Free mode's tap can still place several blocks (the new block becomes the hit surface). Strokes now use every mouse-move event and bisect the screen path until targets leave no gaps (`picking::trace_targets`), so results do not depend on frame rate and no world-space line is drawn; QA that fast strokes no longer phase through volumes.
- **Unknown block names** (typos in patterns) create a new block type with a hash-derived placeholder colour, and export under that name (external tools show air or ignore it). The UI warns "Creates new block type". Real validation needs a block registry: plan is to read valid names from the texture pack folder's `blockstates/` at milestone 7.
- **Selection overlay is meshed like solid blocks**: from inside a selection it is back-face culled like solid blocks. Not wrong, just noted; revisit if it confuses.
- **No antialiasing** yet (eframe `multisampling` / MSAA pipeline sample counts). Deferred to a later patch.
- Overlay depth: coplanar overlay faces use a pipeline depth bias (-8, slope -1.5) plus a 0.0005 lift; a larger lift made block edges peek through at exposed edges.
- Placeholder palette caps real block ids at 2040 (see Dev notes); imports beyond that show magenta until texture packs.
- Pencil only starts against existing blocks.
- Undo memory is uncapped (see open decisions).

## Build order (each step leaves a working app)

1. Core grid + palette storage + mesher + cube rendering
2. Selection + undo/redo
3. Brushes (shape + mask split)
4. `.schem` import AND export
5. One procedural generator end-to-end
6. Layers
7. Texture packs (cubes, then models)
8. Image/OBJ -> blocks, OBJ export
9. Lua scripting
10. Node graph, splines, parametric shapes

MVP = steps 1-4. Then add the public README.

## Not yet decided

- Whether Amulet (MCEdit successor) licensing permits reuse: sources conflict, verify before relying.
- goPaint/goBrush features: not researched.
