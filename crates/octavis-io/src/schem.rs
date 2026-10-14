//! Sponge schematic (`.schem`) reading and writing.
//!
//! Reads versions 1-3 (v3 nests its data under a `Blocks` compound) and
//! writes version 2, which WorldEdit, FAWE, Amulet and most other tools
//! understand. All Minecraft-format knowledge lives here; the core only
//! sees block states and cells.
//!
//! Block entities (chest contents, signs, ...) and entities are not
//! supported by the core yet, so they are dropped on import and the
//! schematic's `warnings` say how many.

use std::collections::HashMap;
use std::fmt;
use std::io::{Read, Write};

use fastnbt::{ByteArray, IntArray, SerOpts, Value};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use glam::IVec3;
use octavis_core::{BlockId, BlockState, BlockTable, World};

/// DataVersion written to files (Minecraft 1.21.1). Readers upgrade older
/// data, so a slightly old value is safe; a newer one than the reader knows
/// is not.
pub const DEFAULT_DATA_VERSION: i32 = 3955;
/// Largest side a schematic can have (Width/Height/Length are 16-bit).
pub const MAX_SIDE: i32 = 65_535;
/// Refuse regions bigger than this many cells (a `u32` each in memory).
pub const MAX_CELLS: i64 = 64_000_000;

#[derive(Debug)]
pub struct SchemError(pub String);

impl fmt::Display for SchemError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SchemError {}

fn err<T>(msg: impl Into<String>) -> Result<T, SchemError> {
    Err(SchemError(msg.into()))
}

/// A schematic as plain data: a box of palette indices. `indices` runs
/// x fastest, then z, then y (the Sponge order).
#[derive(Clone, Debug, PartialEq)]
pub struct Schematic {
    pub size: IVec3,
    pub palette: Vec<BlockState>,
    pub indices: Vec<u32>,
    /// Things that were lost or ignored while reading.
    pub warnings: Vec<String>,
}

fn is_air(state: &BlockState) -> bool {
    matches!(state.name.as_str(), "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air")
}

impl Schematic {
    /// Non-air cells as world changes, with the schematic's minimum corner
    /// at `origin`. Block states are interned into `blocks`.
    pub fn place(&self, blocks: &mut BlockTable, origin: IVec3) -> Vec<(IVec3, BlockId)> {
        let ids: Vec<BlockId> = self
            .palette
            .iter()
            .map(|s| if is_air(s) { BlockId::AIR } else { blocks.intern(s.clone()) })
            .collect();
        let (w, l) = (self.size.x as usize, self.size.z as usize);
        let mut out = Vec::new();
        for (i, &p) in self.indices.iter().enumerate() {
            let id = ids[p as usize];
            if id != BlockId::AIR {
                let local = IVec3::new((i % w) as i32, (i / (w * l)) as i32, ((i / w) % l) as i32);
                out.push((origin + local, id));
            }
        }
        out
    }

    /// Copies the box `min..=max` out of the world. Cells for which
    /// `include` is false become air.
    pub fn from_world(
        world: &World,
        min: IVec3,
        max: IVec3,
        include: impl Fn(IVec3) -> bool,
    ) -> Result<Schematic, SchemError> {
        let size = max - min + IVec3::ONE;
        if size.min_element() < 1 {
            return err("empty region");
        }
        if size.max_element() > MAX_SIDE {
            return err(format!("region is {} blocks on its longest side; the limit is {MAX_SIDE}", size.max_element()));
        }
        let cells = size.x as i64 * size.y as i64 * size.z as i64;
        if cells > MAX_CELLS {
            return err(format!("region has {cells} cells; the limit is {MAX_CELLS}"));
        }
        let mut palette = vec![BlockState::new("minecraft:air")];
        let mut seen: HashMap<BlockId, u32> = HashMap::from([(BlockId::AIR, 0)]);
        let mut indices = Vec::with_capacity(cells as usize);
        for y in 0..size.y {
            for z in 0..size.z {
                for x in 0..size.x {
                    let p = min + IVec3::new(x, y, z);
                    let id = if include(p) { world.get(p) } else { BlockId::AIR };
                    let idx = *seen.entry(id).or_insert_with(|| {
                        let state = world
                            .blocks
                            .get(id)
                            .cloned()
                            .unwrap_or_else(|| BlockState::new("minecraft:air"));
                        palette.push(state);
                        palette.len() as u32 - 1
                    });
                    indices.push(idx);
                }
            }
        }
        Ok(Schematic { size, palette, indices, warnings: Vec::new() })
    }
}

// ----------------------------------------------------- block state text

/// `minecraft:oak_stairs[facing=north,half=bottom]`
pub fn state_to_string(state: &BlockState) -> String {
    if state.properties.is_empty() {
        return state.name.clone();
    }
    let props: Vec<String> = state.properties.iter().map(|(k, v)| format!("{k}={v}")).collect();
    format!("{}[{}]", state.name, props.join(","))
}

pub fn parse_state(text: &str) -> Result<BlockState, SchemError> {
    let (name, props) = match text.split_once('[') {
        Some((n, rest)) => match rest.strip_suffix(']') {
            Some(p) => (n, Some(p)),
            None => return err(format!("bad block state '{text}'")),
        },
        None => (text, None),
    };
    if name.is_empty() {
        return err("block state with no name");
    }
    let name = if name.contains(':') { name.to_string() } else { format!("minecraft:{name}") };
    let mut state = BlockState::new(name);
    for pair in props.into_iter().flat_map(|p| p.split(',')).filter(|p| !p.is_empty()) {
        let Some((k, v)) = pair.split_once('=') else {
            return err(format!("bad property '{pair}' in '{text}'"));
        };
        state = state.with(k, v);
    }
    Ok(state)
}

// --------------------------------------------------------------- varint

fn write_varint(out: &mut Vec<i8>, mut v: u32) {
    loop {
        let byte = (v & 0x7f) as u8;
        v >>= 7;
        if v == 0 {
            out.push(byte as i8);
            return;
        }
        out.push((byte | 0x80) as i8);
    }
}

fn read_varints(data: &[i8], count: usize) -> Result<Vec<u32>, SchemError> {
    let mut out = Vec::with_capacity(count);
    let mut bytes = data.iter().map(|&b| b as u8);
    while out.len() < count {
        let (mut value, mut shift) = (0u32, 0u32);
        loop {
            let Some(b) = bytes.next() else {
                return err(format!("block data ends early ({} of {count} cells)", out.len()));
            };
            value |= ((b & 0x7f) as u32) << shift;
            if b & 0x80 == 0 {
                break;
            }
            shift += 7;
            if shift >= 35 {
                return err("corrupt block data (varint too long)");
            }
        }
        out.push(value);
    }
    Ok(out)
}

// -------------------------------------------------------------- reading

fn gunzip_if_needed(bytes: &[u8]) -> Result<Vec<u8>, SchemError> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        GzDecoder::new(bytes)
            .read_to_end(&mut out)
            .map_err(|e| SchemError(format!("not a valid gzip file: {e}")))?;
        Ok(out)
    } else {
        Ok(bytes.to_vec())
    }
}

fn compound(v: Value, what: &str) -> Result<HashMap<String, Value>, SchemError> {
    match v {
        Value::Compound(c) => Ok(c),
        _ => err(format!("'{what}' is not a compound")),
    }
}

fn take(map: &mut HashMap<String, Value>, key: &str) -> Result<Value, SchemError> {
    map.remove(key).ok_or_else(|| SchemError(format!("missing '{key}'")))
}

fn int(v: Value, what: &str) -> Result<i32, SchemError> {
    match v {
        Value::Int(i) => Ok(i),
        Value::Short(i) => Ok(i as i32),
        Value::Byte(i) => Ok(i as i32),
        _ => err(format!("'{what}' is not a number")),
    }
}

fn side(map: &mut HashMap<String, Value>, key: &str) -> Result<i32, SchemError> {
    match take(map, key)? {
        // Stored as an unsigned short.
        Value::Short(s) => Ok(s as u16 as i32),
        Value::Int(i) if i >= 0 => Ok(i),
        _ => err(format!("'{key}' is not a valid size")),
    }
}

fn list_len(map: &mut HashMap<String, Value>, key: &str) -> usize {
    match map.remove(key) {
        Some(Value::List(l)) => l.len(),
        _ => 0,
    }
}

pub fn read_schem(bytes: &[u8]) -> Result<Schematic, SchemError> {
    let raw = gunzip_if_needed(bytes)?;
    // The NBT library's default sequence limit (100M) would reject big files
    // with a vague message; lift it so the cell limit below explains itself.
    let value: Value =
        fastnbt::from_bytes_with_opts(&raw, fastnbt::DeOpts::new().max_seq_len(i32::MAX as usize)).map_err(|e| SchemError(format!("not a valid NBT file: {e}")))?;
    let mut root = compound(value, "root")?;
    // Old MCEdit / Schematica files (numeric block ids) often carry a
    // .schem extension too.
    if crate::legacy::is_legacy(&root) {
        return crate::legacy::read_legacy(root);
    }
    // v3 (and some v2 writers) wrap everything in a "Schematic" compound.
    if let Some(inner) = root.remove("Schematic") {
        root = compound(inner, "Schematic")?;
    }

    let version = int(take(&mut root, "Version")?, "Version")?;
    if !(1..=3).contains(&version) {
        return err(format!("unsupported schematic version {version} (supported: 1-3)"));
    }
    let size = IVec3::new(side(&mut root, "Width")?, side(&mut root, "Height")?, side(&mut root, "Length")?);
    let cells = size.x as i64 * size.y as i64 * size.z as i64;
    if cells > MAX_CELLS {
        return err(format!("schematic has {cells} cells; the limit is {MAX_CELLS}"));
    }

    let mut warnings = Vec::new();
    let entities = list_len(&mut root, "Entities");
    let (palette_value, data_value, block_entities) = if version >= 3 {
        let mut blocks = compound(take(&mut root, "Blocks")?, "Blocks")?;
        let be = list_len(&mut blocks, "BlockEntities");
        (take(&mut blocks, "Palette")?, take(&mut blocks, "Data")?, be)
    } else {
        let be = list_len(&mut root, "BlockEntities");
        (take(&mut root, "Palette")?, take(&mut root, "BlockData")?, be)
    };
    if block_entities > 0 {
        warnings.push(format!("{block_entities} block entities (chests, signs, ...) were not imported"));
    }
    if entities > 0 {
        warnings.push(format!("{entities} entities were not imported"));
    }

    let palette_map = compound(palette_value, "Palette")?;
    let mut palette: Vec<Option<BlockState>> = vec![None; palette_map.len()];
    for (text, index) in palette_map {
        let i = int(index, "palette index")?;
        let slot = usize::try_from(i)
            .ok()
            .and_then(|i| palette.get_mut(i))
            .ok_or_else(|| SchemError(format!("palette index {i} for '{text}' is out of range")))?;
        if slot.replace(parse_state(&text)?).is_some() {
            return err(format!("palette index {i} is used twice"));
        }
    }
    let palette: Vec<BlockState> =
        palette.into_iter().collect::<Option<_>>().ok_or_else(|| SchemError("palette has gaps".into()))?;

    let Value::ByteArray(data) = data_value else { return err("block data is not a byte array") };
    let indices = read_varints(&data, cells as usize)?;
    if let Some(bad) = indices.iter().find(|&&i| i as usize >= palette.len()) {
        return err(format!("block data refers to palette entry {bad}, but the palette has {}", palette.len()));
    }
    Ok(Schematic { size, palette, indices, warnings })
}

// -------------------------------------------------------------- writing

/// Writes a gzip-compressed Sponge v2 schematic.
pub fn write_schem(s: &Schematic, data_version: i32) -> Result<Vec<u8>, SchemError> {
    if s.size.max_element() > MAX_SIDE || s.size.min_element() < 1 {
        return err("schematic size is out of range");
    }
    let mut data = Vec::with_capacity(s.indices.len());
    for &i in &s.indices {
        write_varint(&mut data, i);
    }
    let palette: HashMap<String, Value> = s
        .palette
        .iter()
        .enumerate()
        .map(|(i, state)| (state_to_string(state), Value::Int(i as i32)))
        .collect();
    let short = |v: i32| Value::Short(v as u16 as i16);
    let root = HashMap::from([
        ("Version".to_string(), Value::Int(2)),
        ("DataVersion".to_string(), Value::Int(data_version)),
        ("Width".to_string(), short(s.size.x)),
        ("Height".to_string(), short(s.size.y)),
        ("Length".to_string(), short(s.size.z)),
        ("Offset".to_string(), Value::IntArray(IntArray::new(vec![0, 0, 0]))),
        ("PaletteMax".to_string(), Value::Int(s.palette.len() as i32)),
        ("Palette".to_string(), Value::Compound(palette)),
        ("BlockData".to_string(), Value::ByteArray(ByteArray::new(data))),
    ]);
    // Readers expect the root tag to be named "Schematic".
    let nbt = fastnbt::to_bytes_with_opts(&Value::Compound(root), SerOpts::new().root_name("Schematic"))
        .map_err(|e| SchemError(format!("could not encode NBT: {e}")))?;
    let mut gz = GzEncoder::new(Vec::new(), Compression::default());
    gz.write_all(&nbt).map_err(|e| SchemError(e.to_string()))?;
    gz.finish().map_err(|e| SchemError(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> (World, Vec<BlockId>) {
        let mut w = World::new();
        let stone = w.blocks.intern(BlockState::new("minecraft:stone"));
        let stairs = w.blocks.intern(
            BlockState::new("minecraft:oak_stairs").with("facing", "north").with("half", "bottom"),
        );
        let modded = w.blocks.intern(BlockState::new("create:cogwheel"));
        w.set(IVec3::new(-2, 5, 7), stone);
        w.set(IVec3::new(0, 5, 8), stairs);
        w.set(IVec3::new(1, 6, 9), modded);
        (w, vec![stone, stairs, modded])
    }

    #[test]
    fn state_text_roundtrip() {
        let s = BlockState::new("minecraft:oak_stairs").with("half", "top").with("facing", "east");
        let text = state_to_string(&s);
        assert_eq!(text, "minecraft:oak_stairs[facing=east,half=top]");
        assert_eq!(parse_state(&text).unwrap(), s);
        assert_eq!(parse_state("stone").unwrap(), BlockState::new("minecraft:stone"));
        for bad in ["", "a[b", "a[b]c", "a[x]"] {
            assert!(parse_state(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn varints_roundtrip_including_multibyte() {
        let values = [0, 1, 127, 128, 300, 16_383, 16_384, 2_000_000];
        let mut bytes = Vec::new();
        for v in values {
            write_varint(&mut bytes, v);
        }
        assert_eq!(read_varints(&bytes, values.len()).unwrap(), values);
        assert!(read_varints(&bytes[..bytes.len() - 1], values.len()).is_err());
    }

    #[test]
    fn world_roundtrips_through_a_file() {
        let (w, ids) = world();
        let min = IVec3::new(-2, 5, 7);
        let max = IVec3::new(1, 6, 9);
        let schem = Schematic::from_world(&w, min, max, |_| true).unwrap();
        assert_eq!(schem.size, IVec3::new(4, 2, 3));

        let bytes = write_schem(&schem, DEFAULT_DATA_VERSION).unwrap();
        assert_eq!(&bytes[..2], &[0x1f, 0x8b]);
        // Readers like WorldEdit require the root compound to be named "Schematic".
        let raw = gunzip_if_needed(&bytes).unwrap();
        assert_eq!(&raw[..12], b"\x0a\x00\x09Schematic");
        let back = read_schem(&bytes).unwrap();
        assert_eq!(back.size, schem.size);
        assert!(back.warnings.is_empty());

        // Place into a fresh world at a different origin and compare.
        let mut fresh = World::new();
        let origin = IVec3::new(100, 0, -50);
        for (p, b) in back.place(&mut fresh.blocks, origin) {
            fresh.set(p, b);
        }
        let name = |world: &World, p: IVec3| world.blocks.get(world.get(p)).cloned();
        for (p, expect) in [
            (IVec3::new(-2, 5, 7), ids[0]),
            (IVec3::new(0, 5, 8), ids[1]),
            (IVec3::new(1, 6, 9), ids[2]),
        ] {
            assert_eq!(name(&fresh, origin + (p - min)), w.blocks.get(expect).cloned());
        }
        assert_eq!(fresh.get(origin + IVec3::X), BlockId::AIR);
        let solid = (0..4).flat_map(|x| (0..2).flat_map(move |y| (0..3).map(move |z| IVec3::new(x, y, z))));
        assert_eq!(solid.filter(|&p| fresh.get(origin + p) != BlockId::AIR).count(), 3);
    }

    #[test]
    fn include_filter_blanks_cells() {
        let (w, _) = world();
        let schem =
            Schematic::from_world(&w, IVec3::new(-2, 5, 7), IVec3::new(1, 6, 9), |p| p.x < 0).unwrap();
        let mut t = BlockTable::default();
        assert_eq!(schem.place(&mut t, IVec3::ZERO).len(), 1);
    }

    #[test]
    fn large_palette_uses_multibyte_indices() {
        let mut w = World::new();
        for i in 0..300 {
            let id = w.blocks.intern(BlockState::new(format!("mod:block_{i}")));
            w.set(IVec3::new(i % 20, 0, i / 20), id);
        }
        let schem = Schematic::from_world(&w, IVec3::ZERO, IVec3::new(19, 0, 14), |_| true).unwrap();
        assert_eq!(schem.palette.len(), 301);
        let back = read_schem(&write_schem(&schem, 3955).unwrap()).unwrap();
        assert_eq!(back.indices, schem.indices);
        assert_eq!(back.palette, schem.palette);
    }

    fn v3_file(extra_air: &str) -> Vec<u8> {
        let palette = HashMap::from([
            ("minecraft:stone".to_string(), Value::Int(0)),
            (extra_air.to_string(), Value::Int(1)),
        ]);
        let blocks = HashMap::from([
            ("Palette".to_string(), Value::Compound(palette)),
            ("Data".to_string(), Value::ByteArray(ByteArray::new(vec![0, 1]))),
            ("BlockEntities".to_string(), Value::List(vec![Value::Compound(HashMap::new())])),
        ]);
        let schematic = HashMap::from([
            ("Version".to_string(), Value::Int(3)),
            ("DataVersion".to_string(), Value::Int(3955)),
            ("Width".to_string(), Value::Short(2)),
            ("Height".to_string(), Value::Short(1)),
            ("Length".to_string(), Value::Short(1)),
            ("Blocks".to_string(), Value::Compound(blocks)),
        ]);
        let root = HashMap::from([("Schematic".to_string(), Value::Compound(schematic))]);
        fastnbt::to_bytes(&Value::Compound(root)).unwrap() // ungzipped on purpose
    }

    #[test]
    fn reads_v3_layout_and_treats_air_variants_as_air() {
        let s = read_schem(&v3_file("minecraft:cave_air")).unwrap();
        assert_eq!(s.size, IVec3::new(2, 1, 1));
        assert_eq!(s.warnings.len(), 1);
        assert!(s.warnings[0].contains("block entities"));
        let mut t = BlockTable::default();
        let cells = s.place(&mut t, IVec3::new(10, 0, 0));
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0].0, IVec3::new(10, 0, 0));
    }

    #[test]
    fn rejects_bad_files_with_a_message() {
        assert!(read_schem(b"definitely not nbt").is_err());
        assert!(read_schem(&[]).is_err());
        let (w, _) = world();
        let schem = Schematic::from_world(&w, IVec3::new(-2, 5, 7), IVec3::new(1, 6, 9), |_| true).unwrap();
        let mut bytes = write_schem(&schem, 3955).unwrap();
        bytes.truncate(bytes.len() / 2);
        assert!(read_schem(&bytes).is_err());
    }

    #[test]
    fn region_limits() {
        let (w, _) = world();
        assert!(Schematic::from_world(&w, IVec3::ZERO, IVec3::new(70_000, 0, 0), |_| true).is_err());
        assert!(Schematic::from_world(&w, IVec3::new(5, 0, 0), IVec3::ZERO, |_| true).is_err());
    }
}
