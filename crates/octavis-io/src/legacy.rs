//! Legacy MCEdit / Schematica `.schematic` files (numeric block ids plus a
//! 4-bit data value), which many sites still serve with a `.schem`
//! extension. Reading translates the pre-1.13 ids to modern block names.
//!
//! The translation covers every vanilla block up to 1.12. Variants (wood
//! type, colour, stone kind) and the common orientation data (stairs, slabs,
//! logs, torches, chests, furnaces, ladders) are converted; rarer state
//! (door hinges, redstone power, banners, ...) is dropped, and tile
//! entities (chest contents, signs) are not read.

use std::collections::HashMap;

use fastnbt::Value;
use glam::IVec3;
use octavis_core::BlockState;

use crate::schem::{SchemError, Schematic, MAX_CELLS};

const COLORS: [&str; 16] = [
    "white", "orange", "magenta", "light_blue", "yellow", "lime", "pink", "gray", "light_gray",
    "cyan", "purple", "blue", "brown", "green", "red", "black",
];
const WOODS: [&str; 6] = ["oak", "spruce", "birch", "jungle", "acacia", "dark_oak"];
const FACING_STAIRS: [&str; 4] = ["east", "west", "south", "north"];
const FACING_BLOCK: [&str; 4] = ["north", "south", "west", "east"]; // data 2..=5

fn simple(name: &str) -> BlockState {
    BlockState::new(format!("minecraft:{name}"))
}

fn pick<'a>(list: &[&'a str], i: usize) -> &'a str {
    list[i % list.len()]
}

/// `data` 2..=5 as a horizontal facing, if it is one.
fn facing_2_5(data: u8) -> Option<&'static str> {
    (2..=5).contains(&data).then(|| FACING_BLOCK[(data - 2) as usize])
}

fn stairs(name: &str, data: u8) -> BlockState {
    simple(name)
        .with("facing", FACING_STAIRS[(data & 3) as usize])
        .with("half", if data & 4 != 0 { "top" } else { "bottom" })
        .with("shape", "straight")
}

fn slab(name: &str, data: u8) -> BlockState {
    simple(name).with("type", if data & 8 != 0 { "top" } else { "bottom" })
}

fn log(wood: &str, data: u8) -> BlockState {
    match (data >> 2) & 3 {
        0 => simple(&format!("{wood}_log")).with("axis", "y"),
        1 => simple(&format!("{wood}_log")).with("axis", "x"),
        2 => simple(&format!("{wood}_log")).with("axis", "z"),
        _ => simple(&format!("{wood}_wood")).with("axis", "y"),
    }
}

fn colored(suffix: &str, data: u8) -> BlockState {
    simple(&format!("{}_{suffix}", pick(&COLORS, (data & 15) as usize)))
}

/// Translates a pre-1.13 block id and data value, or `None` for ids this
/// table does not know.
pub fn legacy_state(id: u16, data: u8) -> Option<BlockState> {
    let d = data as usize;
    let s = |n: &str| Some(simple(n));
    Some(match id {
        0 => simple("air"),
        1 => simple(pick(
            &["stone", "granite", "polished_granite", "diorite", "polished_diorite", "andesite", "polished_andesite"],
            d % 7,
        )),
        2 => return s("grass_block"),
        3 => simple(pick(&["dirt", "coarse_dirt", "podzol"], d % 3)),
        4 => return s("cobblestone"),
        5 => simple(&format!("{}_planks", pick(&WOODS, d & 7))),
        6 => simple(&format!("{}_sapling", pick(&WOODS, d & 7))),
        7 => return s("bedrock"),
        8 | 9 => simple("water").with("level", (data & 15).to_string()),
        10 | 11 => simple("lava").with("level", (data & 15).to_string()),
        12 => simple(pick(&["sand", "red_sand"], d % 2)),
        13 => return s("gravel"),
        14 => return s("gold_ore"),
        15 => return s("iron_ore"),
        16 => return s("coal_ore"),
        17 => log(pick(&WOODS, d & 3), data),
        18 | 161 => {
            let base = if id == 18 { d & 3 } else { 4 + (d & 1) };
            simple(&format!("{}_leaves", pick(&WOODS, base)))
        }
        19 => simple(pick(&["sponge", "wet_sponge"], d % 2)),
        20 => return s("glass"),
        21 => return s("lapis_ore"),
        22 => return s("lapis_block"),
        23 => simple("dispenser").with("facing", facing_2_5(data).unwrap_or("north")),
        24 => simple(pick(&["sandstone", "chiseled_sandstone", "cut_sandstone"], d % 3)),
        25 => return s("note_block"),
        26 => return s("red_bed"),
        27 => return s("powered_rail"),
        28 => return s("detector_rail"),
        29 => return s("sticky_piston"),
        30 => return s("cobweb"),
        31 => simple(pick(&["dead_bush", "short_grass", "fern"], d % 3)),
        32 => return s("dead_bush"),
        33 => return s("piston"),
        34 => return s("piston_head"),
        35 => colored("wool", data),
        36 => return s("moving_piston"),
        37 => return s("dandelion"),
        38 => simple(pick(
            &[
                "poppy", "blue_orchid", "allium", "azure_bluet", "red_tulip", "orange_tulip",
                "white_tulip", "pink_tulip", "oxeye_daisy",
            ],
            d % 9,
        )),
        39 => return s("brown_mushroom"),
        40 => return s("red_mushroom"),
        41 => return s("gold_block"),
        42 => return s("iron_block"),
        43 => simple(pick(
            &[
                "smooth_stone", "sandstone", "oak_planks", "cobblestone", "bricks", "stone_bricks",
                "nether_bricks", "quartz_block",
            ],
            d & 7,
        )),
        44 => slab(
            pick(
                &[
                    "smooth_stone_slab", "sandstone_slab", "petrified_oak_slab", "cobblestone_slab",
                    "brick_slab", "stone_brick_slab", "nether_brick_slab", "quartz_slab",
                ],
                d & 7,
            ),
            data,
        ),
        45 => return s("bricks"),
        46 => return s("tnt"),
        47 => return s("bookshelf"),
        48 => return s("mossy_cobblestone"),
        49 => return s("obsidian"),
        50 | 75 | 76 => {
            let facing = (1..=4).contains(&data).then(|| ["east", "west", "south", "north"][(data - 1) as usize]);
            let lit = (id == 76).to_string();
            match (id == 50, facing) {
                (true, Some(f)) => simple("wall_torch").with("facing", f),
                (true, None) => simple("torch"),
                (false, Some(f)) => simple("redstone_wall_torch").with("facing", f).with("lit", lit),
                (false, None) => simple("redstone_torch").with("lit", lit),
            }
        }
        51 => return s("fire"),
        52 => return s("spawner"),
        53 => stairs("oak_stairs", data),
        54 => simple("chest").with("facing", facing_2_5(data).unwrap_or("north")),
        55 => return s("redstone_wire"),
        56 => return s("diamond_ore"),
        57 => return s("diamond_block"),
        58 => return s("crafting_table"),
        59 => simple("wheat").with("age", (data & 7).to_string()),
        60 => simple("farmland").with("moisture", (data & 7).to_string()),
        61 => simple("furnace").with("facing", facing_2_5(data).unwrap_or("north")).with("lit", "false"),
        62 => simple("furnace").with("facing", facing_2_5(data).unwrap_or("north")).with("lit", "true"),
        63 => return s("oak_sign"),
        64 | 71 | 193..=197 => {
            let name = match id {
                64 => "oak_door",
                71 => "iron_door",
                193 => "spruce_door",
                194 => "birch_door",
                195 => "jungle_door",
                196 => "acacia_door",
                _ => "dark_oak_door",
            };
            simple(name).with("half", if data & 8 != 0 { "upper" } else { "lower" })
        }
        65 => simple("ladder").with("facing", facing_2_5(data).unwrap_or("north")),
        66 => return s("rail"),
        67 => stairs("cobblestone_stairs", data),
        68 => return s("oak_wall_sign"),
        69 => return s("lever"),
        70 => return s("stone_pressure_plate"),
        72 => return s("oak_pressure_plate"),
        73 => return s("redstone_ore"),
        74 => simple("redstone_ore").with("lit", "true"),
        77 => return s("stone_button"),
        78 => simple("snow").with("layers", ((data & 7) + 1).to_string()),
        79 => return s("ice"),
        80 => return s("snow_block"),
        81 => return s("cactus"),
        82 => return s("clay"),
        83 => return s("sugar_cane"),
        84 => return s("jukebox"),
        85 => return s("oak_fence"),
        86 => return s("pumpkin"),
        87 => return s("netherrack"),
        88 => return s("soul_sand"),
        89 => return s("glowstone"),
        90 => return s("nether_portal"),
        91 => return s("jack_o_lantern"),
        92 => return s("cake"),
        93 => return s("repeater"),
        94 => simple("repeater").with("powered", "true"),
        95 => colored("stained_glass", data),
        96 => return s("oak_trapdoor"),
        97 => simple(pick(
            &[
                "infested_stone", "infested_cobblestone", "infested_stone_bricks",
                "infested_mossy_stone_bricks", "infested_cracked_stone_bricks",
                "infested_chiseled_stone_bricks",
            ],
            d % 6,
        )),
        98 => simple(pick(
            &["stone_bricks", "mossy_stone_bricks", "cracked_stone_bricks", "chiseled_stone_bricks"],
            d & 3,
        )),
        99 => return s("brown_mushroom_block"),
        100 => return s("red_mushroom_block"),
        101 => return s("iron_bars"),
        102 => return s("glass_pane"),
        103 => return s("melon"),
        104 => return s("pumpkin_stem"),
        105 => return s("melon_stem"),
        106 => return s("vine"),
        107 => return s("oak_fence_gate"),
        108 => stairs("brick_stairs", data),
        109 => stairs("stone_brick_stairs", data),
        110 => return s("mycelium"),
        111 => return s("lily_pad"),
        112 => return s("nether_bricks"),
        113 => return s("nether_brick_fence"),
        114 => stairs("nether_brick_stairs", data),
        115 => return s("nether_wart"),
        116 => return s("enchanting_table"),
        117 => return s("brewing_stand"),
        118 => return s("cauldron"),
        119 => return s("end_portal"),
        120 => return s("end_portal_frame"),
        121 => return s("end_stone"),
        122 => return s("dragon_egg"),
        123 => return s("redstone_lamp"),
        124 => simple("redstone_lamp").with("lit", "true"),
        125 => simple(&format!("{}_planks", pick(&WOODS, d & 7))),
        126 => slab(&format!("{}_slab", pick(&WOODS, d & 7)), data),
        127 => return s("cocoa"),
        128 => stairs("sandstone_stairs", data),
        129 => return s("emerald_ore"),
        130 => simple("ender_chest").with("facing", facing_2_5(data).unwrap_or("north")),
        131 => return s("tripwire_hook"),
        132 => return s("tripwire"),
        133 => return s("emerald_block"),
        134 => stairs("spruce_stairs", data),
        135 => stairs("birch_stairs", data),
        136 => stairs("jungle_stairs", data),
        137 => return s("command_block"),
        138 => return s("beacon"),
        139 => simple(pick(&["cobblestone_wall", "mossy_cobblestone_wall"], d % 2)),
        140 => return s("flower_pot"),
        141 => return s("carrots"),
        142 => return s("potatoes"),
        143 => return s("oak_button"),
        144 => return s("skeleton_skull"),
        145 => return s("anvil"),
        146 => simple("trapped_chest").with("facing", facing_2_5(data).unwrap_or("north")),
        147 => return s("light_weighted_pressure_plate"),
        148 => return s("heavy_weighted_pressure_plate"),
        149 | 150 => return s("comparator"),
        151 | 178 => return s("daylight_detector"),
        152 => return s("redstone_block"),
        153 => return s("nether_quartz_ore"),
        154 => return s("hopper"),
        155 => simple(pick(&["quartz_block", "chiseled_quartz_block", "quartz_pillar"], (d % 5).min(2))),
        156 => stairs("quartz_stairs", data),
        157 => return s("activator_rail"),
        158 => return s("dropper"),
        159 => colored("terracotta", data),
        160 => colored("stained_glass_pane", data),
        162 => log(pick(&["acacia", "dark_oak"], d & 1), data),
        163 => stairs("acacia_stairs", data),
        164 => stairs("dark_oak_stairs", data),
        165 => return s("slime_block"),
        166 => return s("barrier"),
        167 => return s("iron_trapdoor"),
        168 => simple(pick(&["prismarine", "prismarine_bricks", "dark_prismarine"], d % 3)),
        169 => return s("sea_lantern"),
        170 => return s("hay_block"),
        171 => colored("carpet", data),
        172 => return s("terracotta"),
        173 => return s("coal_block"),
        174 => return s("packed_ice"),
        175 => simple(pick(&["sunflower", "lilac", "tall_grass", "large_fern", "rose_bush", "peony"], d % 6)),
        176 => return s("white_banner"),
        177 => return s("white_wall_banner"),
        179 => simple(pick(&["red_sandstone", "chiseled_red_sandstone", "cut_red_sandstone"], d % 3)),
        180 => stairs("red_sandstone_stairs", data),
        181 => return s("red_sandstone"),
        182 => slab("red_sandstone_slab", data),
        183 => return s("spruce_fence_gate"),
        184 => return s("birch_fence_gate"),
        185 => return s("jungle_fence_gate"),
        186 => return s("dark_oak_fence_gate"),
        187 => return s("acacia_fence_gate"),
        188 => return s("spruce_fence"),
        189 => return s("birch_fence"),
        190 => return s("jungle_fence"),
        191 => return s("dark_oak_fence"),
        192 => return s("acacia_fence"),
        198 => return s("end_rod"),
        199 => return s("chorus_plant"),
        200 => return s("chorus_flower"),
        201 => return s("purpur_block"),
        202 => return s("purpur_pillar"),
        203 => stairs("purpur_stairs", data),
        204 => return s("purpur_block"),
        205 => slab("purpur_slab", data),
        206 => return s("end_stone_bricks"),
        207 => return s("beetroots"),
        208 => return s("dirt_path"),
        209 => return s("end_gateway"),
        210 => return s("repeating_command_block"),
        211 => return s("chain_command_block"),
        212 => return s("frosted_ice"),
        213 => return s("magma_block"),
        214 => return s("nether_wart_block"),
        215 => return s("red_nether_bricks"),
        216 => return s("bone_block"),
        217 => return s("structure_void"),
        218 => return s("observer"),
        219..=234 => simple(&format!("{}_shulker_box", COLORS[(id - 219) as usize])),
        235..=250 => simple(&format!("{}_glazed_terracotta", COLORS[(id - 235) as usize])),
        251 => colored("concrete", data),
        252 => colored("concrete_powder", data),
        255 => return s("structure_block"),
        _ => return None,
    })
}

fn bytes(map: &mut HashMap<String, Value>, key: &str) -> Option<Vec<i8>> {
    match map.remove(key) {
        Some(Value::ByteArray(b)) => Some(b.into_inner()),
        _ => None,
    }
}

fn list_len(map: &mut HashMap<String, Value>, key: &str) -> usize {
    match map.remove(key) {
        Some(Value::List(l)) => l.len(),
        _ => 0,
    }
}

fn dimension(map: &mut HashMap<String, Value>, key: &str) -> Result<i32, SchemError> {
    match map.remove(key) {
        Some(Value::Short(s)) => Ok(s as u16 as i32),
        Some(Value::Int(i)) if i >= 0 => Ok(i),
        _ => Err(SchemError(format!("legacy schematic is missing '{key}'"))),
    }
}

/// True if the root compound looks like a legacy `.schematic`.
pub fn is_legacy(root: &HashMap<String, Value>) -> bool {
    !root.contains_key("Version")
        && matches!(root.get("Blocks"), Some(Value::ByteArray(_)))
        && root.contains_key("Width")
}

pub fn read_legacy(mut root: HashMap<String, Value>) -> Result<Schematic, SchemError> {
    let size = IVec3::new(
        dimension(&mut root, "Width")?,
        dimension(&mut root, "Height")?,
        dimension(&mut root, "Length")?,
    );
    let cells = size.x as i64 * size.y as i64 * size.z as i64;
    if cells > MAX_CELLS {
        return Err(SchemError(format!("schematic has {cells} cells; the limit is {MAX_CELLS}")));
    }
    let cells = cells as usize;
    let blocks = bytes(&mut root, "Blocks").ok_or_else(|| SchemError("legacy schematic has no Blocks".into()))?;
    let data = bytes(&mut root, "Data").ok_or_else(|| SchemError("legacy schematic has no Data".into()))?;
    if blocks.len() < cells || data.len() < cells {
        return Err(SchemError(format!(
            "legacy schematic arrays are too short ({} blocks, {} data, expected {cells})",
            blocks.len(),
            data.len()
        )));
    }
    let add = bytes(&mut root, "AddBlocks");

    let mut warnings = Vec::new();
    let (tile, entities) = (list_len(&mut root, "TileEntities"), list_len(&mut root, "Entities"));
    if tile > 0 {
        warnings.push(format!("{tile} tile entities (chest contents, signs, ...) were not imported"));
    }
    if entities > 0 {
        warnings.push(format!("{entities} entities were not imported"));
    }

    let mut palette: Vec<BlockState> = Vec::new();
    let mut by_state: HashMap<BlockState, u32> = HashMap::new();
    let mut by_key: HashMap<(u16, u8), u32> = HashMap::new();
    let mut unknown: HashMap<u16, usize> = HashMap::new();
    let mut indices = Vec::with_capacity(cells);
    for i in 0..cells {
        let mut id = blocks[i] as u8 as u16;
        if let Some(add) = &add {
            if let Some(&byte) = add.get(i >> 1) {
                let nibble = if i & 1 == 0 { byte & 0x0f } else { (byte >> 4) & 0x0f };
                id |= (nibble as u16) << 8;
            }
        }
        let meta = (data[i] as u8) & 0x0f;
        let index = match by_key.get(&(id, meta)) {
            Some(&k) => k,
            None => {
                let state = legacy_state(id, meta).unwrap_or_else(|| {
                    *unknown.entry(id).or_insert(0) += 1;
                    BlockState::new(format!("octavis:unknown_block_{id}"))
                });
                let k = *by_state.entry(state.clone()).or_insert_with(|| {
                    palette.push(state);
                    palette.len() as u32 - 1
                });
                by_key.insert((id, meta), k);
                k
            }
        };
        indices.push(index);
    }
    if !unknown.is_empty() {
        let mut ids: Vec<u16> = unknown.keys().copied().collect();
        ids.sort_unstable();
        warnings.push(format!("unknown legacy block ids {ids:?} imported as placeholder blocks"));
    }
    warnings.insert(0, "legacy .schematic format (numeric ids) translated to modern block names".into());
    Ok(Schematic { size, palette, indices, warnings })
}

#[cfg(test)]
mod tests {
    use super::*;
    use fastnbt::ByteArray;

    #[test]
    fn common_blocks_translate() {
        assert_eq!(legacy_state(0, 0).unwrap().name, "minecraft:air");
        assert_eq!(legacy_state(1, 3).unwrap().name, "minecraft:diorite");
        assert_eq!(legacy_state(24, 1).unwrap().name, "minecraft:chiseled_sandstone");
        assert_eq!(legacy_state(159, 1).unwrap().name, "minecraft:orange_terracotta");
        assert_eq!(legacy_state(35, 11).unwrap().name, "minecraft:blue_wool");
        assert_eq!(legacy_state(251, 15).unwrap().name, "minecraft:black_concrete");
        assert_eq!(legacy_state(17, 5).unwrap().properties["axis"], "x");
        assert_eq!(legacy_state(162, 1).unwrap().name, "minecraft:dark_oak_log");
        let st = legacy_state(128, 6).unwrap(); // sandstone stairs, south, top
        assert_eq!(st.name, "minecraft:sandstone_stairs");
        assert_eq!((st.properties["facing"].as_str(), st.properties["half"].as_str()), ("south", "top"));
        let st = legacy_state(128, 5).unwrap();
        assert_eq!((st.properties["facing"].as_str(), st.properties["half"].as_str()), ("west", "top"));
        assert_eq!(legacy_state(44, 8 | 1).unwrap().properties["type"], "top");
        assert_eq!(legacy_state(50, 5).unwrap().name, "minecraft:torch");
        assert_eq!(legacy_state(50, 2).unwrap().properties["facing"], "west");
        assert_eq!(legacy_state(54, 4).unwrap().properties["facing"], "west");
        assert!(legacy_state(253, 0).is_none());
    }

    #[test]
    fn every_id_up_to_252_is_known_with_data_zero() {
        for id in 0..=252u16 {
            assert!(legacy_state(id, 0).is_some(), "id {id} missing");
        }
        // Data values never panic for any id.
        for id in 0..=255u16 {
            for data in 0..16 {
                let _ = legacy_state(id, data);
            }
        }
    }

    fn file(blocks: Vec<i8>, data: Vec<i8>, add: Option<Vec<i8>>) -> HashMap<String, Value> {
        let mut root = HashMap::from([
            ("Width".to_string(), Value::Short(2)),
            ("Height".to_string(), Value::Short(1)),
            ("Length".to_string(), Value::Short(2)),
            ("Materials".to_string(), Value::String("Alpha".into())),
            ("Blocks".to_string(), Value::ByteArray(ByteArray::new(blocks))),
            ("Data".to_string(), Value::ByteArray(ByteArray::new(data))),
            ("TileEntities".to_string(), Value::List(vec![Value::Compound(HashMap::new())])),
        ]);
        if let Some(a) = add {
            root.insert("AddBlocks".into(), Value::ByteArray(ByteArray::new(a)));
        }
        root
    }

    #[test]
    fn reads_a_small_legacy_schematic() {
        let root = file(vec![1, 24, 24, -1], vec![0, 0, 1, 0], None); // stone, sandstone, chiseled, id 255
        assert!(is_legacy(&root));
        let s = read_legacy(root).unwrap();
        assert_eq!(s.size, IVec3::new(2, 1, 2));
        assert_eq!(s.indices.len(), 4);
        let names: Vec<&str> = s.indices.iter().map(|&i| s.palette[i as usize].name.as_str()).collect();
        assert_eq!(
            names,
            ["minecraft:stone", "minecraft:sandstone", "minecraft:chiseled_sandstone", "minecraft:structure_block"]
        );
        assert!(s.warnings.iter().any(|w| w.contains("tile entities")));
    }

    #[test]
    fn add_blocks_extends_ids_and_unknown_ids_get_placeholders() {
        // Cell 0: id 1 + (1 << 8) = 257 (unknown); cell 1: id 1 + (2 << 8) = 513.
        let root = file(vec![1, 1, 1, 1], vec![0; 4], Some(vec![0x21, 0x00]));
        let s = read_legacy(root).unwrap();
        let names: Vec<&str> = s.indices.iter().map(|&i| s.palette[i as usize].name.as_str()).collect();
        assert_eq!(names[0], "octavis:unknown_block_257");
        assert_eq!(names[1], "octavis:unknown_block_513");
        assert_eq!(names[2], "minecraft:stone");
        assert!(s.warnings.iter().any(|w| w.contains("unknown legacy block ids")));
    }

    #[test]
    fn short_arrays_are_rejected() {
        assert!(read_legacy(file(vec![1, 1], vec![0, 0], None)).is_err());
    }

    #[test]
    fn sponge_files_are_not_legacy() {
        let mut root = file(vec![1; 4], vec![0; 4], None);
        root.insert("Version".into(), Value::Int(2));
        assert!(!is_legacy(&root));
    }

    /// Reads any schematic named by `OCTAVIS_TEST_SCHEM` and prints a summary:
    /// `OCTAVIS_TEST_SCHEM=path cargo test -p octavis-io -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn reads_the_file_named_by_the_environment() {
        let path = std::env::var("OCTAVIS_TEST_SCHEM").expect("set OCTAVIS_TEST_SCHEM");
        let s = crate::schem::read_schem(&std::fs::read(path).unwrap()).unwrap();
        let solid = s.indices.iter().filter(|&&i| s.palette[i as usize].name != "minecraft:air").count();
        println!("size {:?}, {} palette entries, {solid} solid blocks", s.size, s.palette.len());
        for w in &s.warnings {
            println!("warning: {w}");
        }
        for p in &s.palette {
            println!("  {}", crate::schem::state_to_string(p));
        }
    }
}
