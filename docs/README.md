# Octavis (internal notes)

Standalone native desktop 3D modelling tool for Minecraft builds (schematic scale, not infinite worlds). "MCEdit upgraded, with Blender-style UX." No running Minecraft needed. Keep this file short and current; public README comes at MVP.

Priorities: performance, stability, extensibility.
Platform: Windows / macOS / Linux. Remote: https://github.com/21krstic/Octavis (empty, push verified via dry-run).

## Stack

Rust, winit, wgpu, egui + egui_dock, glam, fastnbt, image, noise-rs, serde, rayon.
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
- Layers: sparse containers, topmost non-air wins; visibility/lock/solo/merge-down.
- Undo: command pattern, one sparse diff per action/stroke.
- **Block registry** (version-independent: properties, shapes, connectivity rules) is a core dependency, not an I/O detail. Needed by fix-connect and type replacer.
- Not an octree (target format is MC's chunk+palette) and not BMesh-based; borrow Blender's UX only.

## Open design decisions (settle before building the relevant milestone)

- **Layers vs. generators (before layers):** non-destructive means a generator layer stores its *recipe* (generator + parameters + seed) plus a cached result, not just cells. Tweak a parameter and it re-runs into that layer without touching layers below. Plain painted layers store only cells. Decide the layer data model to support both from the start.
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

- Crates: `octavis-core` (voxels, no deps but glam), `octavis-mesh` (CPU meshing: culled + per-vertex AO), `octavis-io` (file formats; `.schem` so far), `octavis-app` (eframe + wgpu viewport; file dialogs via rfd).
- Windows: run cargo from **PowerShell**, not Git Bash (Git's `link` shadows MSVC `link.exe`). Needs MSVC Build Tools.
- Vulkan crashes at startup on the dev machine; the app defaults to DX12 on Windows (`WGPU_BACKEND` overrides).
- Run: `cargo run --release`; test: `cargo test --workspace` (both from PowerShell, repo root).
- First full build is slow (~13 min, deps compile at opt-level 3); incremental is seconds.
- Palette in the shader is a 2048-entry placeholder (ids 0-2039 real, 2040-2047 overlays; ids past that show magenta), replaced by texture packs at milestone 7.

## Status

Milestones 1-3 done (QA-accepted; remaining items are in the backlog below). Milestone 4 built, awaiting QA.

1. Core grid, mesher, rendering, orbit camera (zoom-scaled orbit/pan speed).
2. Pencil (Free stroke mode default; Plane-locked kept as an option), select box / sphere / wand (wand ignores air, optional diagonals), undoable selection changes, axis gizmo + origin axes, selection caps (500k cells; sphere radius 49; wand 200k).
3. Brush tool: sphere/cube/cylinder shapes; paint/replace, splatter, overlay, smooth, erode, dilate, erase. Mask language (`air solid exposed surface selected y>=5 <block> ! & | ( )`) and weighted patterns (`70%stone,30%dirt`) live in core `brush.rs`.
   - Strokes dab along a line (no gaps) and pick their target against the world as it was when the stroke began (`Editor::was_solid`), so they never dig or build under themselves. Seed is fixed per stroke, so splatter density is the stroke's coverage.
   - Smooth/erode/dilate run on a dense copy of the area (`Grid`) with a Passes setting (like WorldEdit's smooth brush iterations).
   - `selected` mask term restricts a brush to the selection; "Apply to selection" runs the brush over every selected cell.
   - Not yet: stamp shape (waits for stamps/prefabs), falloff, block properties in patterns/masks (`oak_stairs[facing=north]`), noise/Y-gradient patterns.

4. Built, in QA: `.schem` (Sponge) import + export, File menu (`app/files.rs`, format code in `octavis-io/src/schem.rs`).
   - Reads Sponge v1-3 (gzip or raw NBT); writes v2 (widest compatibility), root tag named `Schematic`, DataVersion 3955 (`DEFAULT_DATA_VERSION`).
   - New scene / Open (replaces scene, frames camera, asks before discarding edits) / Import (pastes at camera target on the highest block there, selects it, one undo step; schematic air does not overwrite) / Export (selection if any, else all non-air blocks).
   - Not supported yet because core has no storage for them: block entities (chest contents, signs) and entities. They are dropped on import with a status-bar warning; export writes none. Schematic `Offset` is ignored.
   - Tested against files built from the spec, not yet against real WorldEdit/FAWE output.
   - Not done: `.nbt` (structure block), `.litematic`, move/transform of pasted selections.

Default scene: flat 480x480 grass/dirt/stone platform; grey viewport background.

## Known issues / backlog (from QA)

- **Performance on weak hardware** (dev laptop is weak): radius-16 smooth and other large operations spike the CPU. Ideas: parallelise planning with rayon, mesh dirty sections off the main thread, faster section hashing, incremental overlay meshing.
- **Erode makes 45/90-degree patterns when repeated**: expected from the rule (it removes corners, so edges chamfer into planes). Possible later fix: randomness or a threshold option.
- **Box select** needs a real fix: corners depend on hovering existing blocks (open-air fallback is only the anchor's horizontal plane). Needs a proper way to define two points in space (e.g. drag a base rectangle, then extrude height).
- **Large sphere selections lag**: the selection overlay is rebuilt (temp world + meshing) every frame of a drag. Fix by incremental/cached overlay meshing or a cheaper overlay for big selections.
- **Pencil**: Free mode's tap can still place several blocks (the new block becomes the hit surface). Line bridging between cursor targets can phase through solid volumes on fast strokes, especially on jumps between near and far surfaces; refine (e.g. interpolate in screen/ray space or against the pre-stroke world, as brushes do).
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
