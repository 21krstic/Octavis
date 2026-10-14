# Octavis

A standalone desktop 3D modelling tool for Minecraft builds. Think MCEdit's job (schematic-scale editing, no running game needed) with Blender-style navigation and a modern editing toolset.

> **Status: early MVP.** The core editing loop works, but expect rough edges. Rendering uses flat placeholder colours for now; texture packs are planned.

Octavis is not affiliated with Mojang or Microsoft and ships no Minecraft assets.

## What works today

- **Unbounded grid** that grows in every direction, with fast chunked storage and culled meshing with ambient occlusion.
- **Pencil:** place and break blocks. Fast strokes leave no gaps and behave the same at any frame rate.
- **Selection:** box, sphere and wand (same-block flood fill, optional diagonals), with replace / add / subtract / intersect. Selection changes are undoable.
- **Brushes:** sphere, cube and cylinder shapes with these modes: paint/replace, splatter, overlay, smooth, erode, dilate and erase.
  - **Masks** decide which cells a brush may change: `stone | dirt & y<10`, `surface`, `exposed`, `air`, `selected`, ...
  - **Patterns** decide what gets written: `70%stone, 30%dirt`.
  - A whole stroke is one undo step.
- **Terrain generator:** seeded fractal-noise heightmaps with configurable scale, roughness, height and block layers (grass / soil / stone, or any patterns). Same seed, same terrain on every machine.
- **Fill / delete** a selection, and **apply a brush to a selection**.
- **Undo / redo** for everything.
- **`.schem` import and export** (Sponge schematics): open a file, import it into the scene, or export a selection or the whole scene. Reads Sponge v1-3 and writes v2, which WorldEdit, FAWE and most other tools understand.

Known limits: block entities (chest contents, signs) and entities are not supported yet and are dropped on import. See [docs/README.md](docs/README.md) for the full backlog.

## Controls

| Input | Action |
| --- | --- |
| Left mouse | Use the active tool |
| Right-drag | Orbit |
| Middle-drag, or Shift + right-drag | Pan |
| Mouse wheel | Zoom |
| Ctrl + left | Break blocks (pencil), erase (brush), subtract (selection) |
| Shift + left | Add to selection |
| Ctrl+Z / Ctrl+Y | Undo / redo |
| Esc | Deselect |

## Building

You need a recent stable [Rust](https://rustup.rs/) toolchain. On Windows you also need the MSVC Build Tools, and you should run cargo from PowerShell or a Developer prompt (not Git Bash, whose `link` shadows MSVC's linker).

```
cargo run --release
```

The first build compiles all dependencies and takes a while. Tests: `cargo test --workspace`.

On Windows the app uses DirectX 12; set `WGPU_BACKEND` to override.

## Roadmap

Roughly in order:

1. More generators (caves, 3D density fields, shapes) and live previews
2. Layers, including non-destructive generator layers
3. Texture packs, loaded from a plain folder you extract yourself (Octavis never bundles Mojang assets; an open placeholder pack is the long-term goal)
4. Image and OBJ to blocks; OBJ export
5. Lua scripting
6. Node-graph generators, splines and parametric shapes

Also planned: reference images, symmetry and array tools, stamps / prefabs, palette tools, `.nbt` and possibly `.litematic` support.

## Project layout

- `octavis-core`: voxel grid, selections, brushes, undo. Depends on nothing but math.
- `octavis-mesh`: CPU meshing.
- `octavis-io`: file formats.
- `octavis-gen`: procedural generators and noise.
- `octavis-app`: the editor (egui + wgpu).

Contributor notes live in [docs/README.md](docs/README.md).

## License

Not yet chosen.
