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

- Crates: `octavis-core` (voxels, no deps but glam), `octavis-mesh` (CPU meshing: culled + per-vertex AO), `octavis-app` (eframe + wgpu viewport).
- Windows: run cargo from **PowerShell**, not Git Bash (Git's `link` shadows MSVC `link.exe`). Needs MSVC Build Tools.
- Vulkan crashes at startup on the dev machine; the app defaults to DX12 on Windows (`WGPU_BACKEND` overrides).
- First full build is slow (~13 min, deps compile at opt-level 3); incremental is seconds.
- Palette in the shader is a 256-entry placeholder (block id & 255), replaced by texture packs at milestone 7.

## Status

Milestone 1 done: core grid + palette storage, culled mesher, cube rendering with orbit camera (right-drag orbit, middle/shift+right pan, wheel zoom) over a demo scene. Next: milestone 2 (selection + undo/redo).

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
