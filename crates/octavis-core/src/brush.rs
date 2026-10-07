//! Brushes. A brush is a [`Shape`] (where), a [`Mask`] (which existing cells
//! may change), a [`Pattern`] (what to write) and a [`BrushMode`] (how).
//! Planning is pure: it reads the world and returns the changes, so smart
//! modes see one consistent snapshot and the caller decides how to apply
//! them (through history, in the app).

use glam::IVec3;

use crate::{BlockId, BlockState, BlockTable, World};

const FACE_DIRS: [IVec3; 6] =
    [IVec3::X, IVec3::NEG_X, IVec3::Y, IVec3::NEG_Y, IVec3::Z, IVec3::NEG_Z];

/// Deterministic per-cell randomness in `[0, 1)`, so a brush stroke can be
/// replayed (and previewed) identically.
fn hash01(pos: IVec3, seed: u32) -> f32 {
    let mut h = seed.wrapping_mul(0x9E37_79B1) ^ 0x85EB_CA6B;
    for v in [pos.x, pos.y, pos.z] {
        h = (h ^ v as u32).wrapping_mul(0xC2B2_AE35);
        h ^= h >> 15;
        h = h.wrapping_mul(0x27D4_EB2F);
        h ^= h >> 13;
    }
    (h >> 8) as f32 / (1u32 << 24) as f32
}

// ---------------------------------------------------------------- shape

/// The volume a brush dab covers, centred on a cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Sphere { radius: f32 },
    /// Cube with side `2 * half + 1`.
    Cube { half: i32 },
    /// Vertical cylinder.
    Cylinder { radius: f32, half_height: i32 },
}

impl Shape {
    pub fn cells(&self, center: IVec3) -> Vec<IVec3> {
        let (rxz, ry) = match *self {
            Shape::Sphere { radius } => (radius.ceil() as i32, radius.ceil() as i32),
            Shape::Cube { half } => (half, half),
            Shape::Cylinder { radius, half_height } => (radius.ceil() as i32, half_height),
        };
        let mut out = Vec::new();
        for y in -ry..=ry {
            for z in -rxz..=rxz {
                for x in -rxz..=rxz {
                    let inside = match *self {
                        Shape::Sphere { radius } => {
                            (x * x + y * y + z * z) as f32 <= radius * radius
                        }
                        Shape::Cube { .. } => true,
                        Shape::Cylinder { radius, .. } => (x * x + z * z) as f32 <= radius * radius,
                    };
                    if inside {
                        out.push(center + IVec3::new(x, y, z));
                    }
                }
            }
        }
        out
    }
}

// ----------------------------------------------------------------- mask

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmp {
    Lt,
    Le,
    Gt,
    Ge,
    Eq,
}

impl Cmp {
    fn test(self, a: i32, b: i32) -> bool {
        match self {
            Cmp::Lt => a < b,
            Cmp::Le => a <= b,
            Cmp::Gt => a > b,
            Cmp::Ge => a >= b,
            Cmp::Eq => a == b,
        }
    }
}

/// Condition on an existing cell. Composable with `!`, `&` and `|`.
///
/// Text form (see [`Mask::parse`]): `air`, `solid`, `exposed` (solid with an
/// air face neighbour), `surface` (solid with air above), `y>=10`, a block
/// name like `stone` or `minecraft:oak_planks`, and `( ) ! & |`.
#[derive(Clone, Debug, PartialEq)]
pub enum Mask {
    Any,
    Air,
    Solid,
    Exposed,
    Surface,
    /// Block name, always with namespace; matches every state of that block.
    Named(String),
    Y(Cmp, i32),
    Not(Box<Mask>),
    And(Vec<Mask>),
    Or(Vec<Mask>),
}

impl Mask {
    pub fn eval(&self, world: &World, pos: IVec3) -> bool {
        match self {
            Mask::Any => true,
            Mask::Air => world.get(pos) == BlockId::AIR,
            Mask::Solid => world.get(pos) != BlockId::AIR,
            Mask::Exposed => {
                world.get(pos) != BlockId::AIR
                    && FACE_DIRS.iter().any(|&d| world.get(pos + d) == BlockId::AIR)
            }
            Mask::Surface => {
                world.get(pos) != BlockId::AIR && world.get(pos + IVec3::Y) == BlockId::AIR
            }
            Mask::Named(name) => {
                world.blocks.get(world.get(pos)).is_some_and(|s| &s.name == name)
            }
            Mask::Y(cmp, v) => cmp.test(pos.y, *v),
            Mask::Not(m) => !m.eval(world, pos),
            Mask::And(ms) => ms.iter().all(|m| m.eval(world, pos)),
            Mask::Or(ms) => ms.iter().any(|m| m.eval(world, pos)),
        }
    }

    /// Block names used by this mask that the table has never seen. They
    /// are legal (they just match nothing) but usually mean a typo.
    pub fn unknown_names(&self, blocks: &BlockTable) -> Vec<String> {
        let mut out = Vec::new();
        self.collect_unknown(blocks, &mut out);
        out
    }

    fn collect_unknown(&self, blocks: &BlockTable, out: &mut Vec<String>) {
        match self {
            Mask::Named(n) if !blocks.has_name(n) => out.push(n.clone()),
            Mask::Not(m) => m.collect_unknown(blocks, out),
            Mask::And(ms) | Mask::Or(ms) => ms.iter().for_each(|m| m.collect_unknown(blocks, out)),
            _ => {}
        }
    }

    /// Parses the text form. Empty text means [`Mask::Any`]. `&` binds
    /// tighter than `|`.
    pub fn parse(text: &str) -> Result<Mask, String> {
        let tokens = lex(text)?;
        if tokens.is_empty() {
            return Ok(Mask::Any);
        }
        let mut p = MaskParser { tokens, at: 0 };
        let mask = p.or()?;
        match p.tokens.get(p.at) {
            None => Ok(mask),
            Some(t) => Err(format!("unexpected {}", t.describe())),
        }
    }
}

/// Adds the default namespace to a bare block name.
pub fn qualify(name: &str) -> String {
    if name.contains(':') { name.to_string() } else { format!("minecraft:{name}") }
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    LParen,
    RParen,
    And,
    Or,
    Not,
    Cmp(Cmp),
    Num(i32),
    Ident(String),
}

impl Token {
    fn describe(&self) -> String {
        match self {
            Token::LParen => "'('".into(),
            Token::RParen => "')'".into(),
            Token::And => "'&'".into(),
            Token::Or => "'|'".into(),
            Token::Not => "'!'".into(),
            Token::Cmp(_) => "comparison".into(),
            Token::Num(n) => format!("number {n}"),
            Token::Ident(s) => format!("'{s}'"),
        }
    }
}

fn lex(text: &str) -> Result<Vec<Token>, String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            ' ' | '\t' => i += 1,
            '(' => {
                out.push(Token::LParen);
                i += 1;
            }
            ')' => {
                out.push(Token::RParen);
                i += 1;
            }
            '&' => {
                out.push(Token::And);
                i += 1;
            }
            '|' => {
                out.push(Token::Or);
                i += 1;
            }
            '!' => {
                out.push(Token::Not);
                i += 1;
            }
            '<' | '>' => {
                let eq = next == Some('=');
                out.push(Token::Cmp(match (c, eq) {
                    ('<', false) => Cmp::Lt,
                    ('<', true) => Cmp::Le,
                    ('>', false) => Cmp::Gt,
                    _ => Cmp::Ge,
                }));
                i += if eq { 2 } else { 1 };
            }
            '=' => {
                out.push(Token::Cmp(Cmp::Eq));
                i += if next == Some('=') { 2 } else { 1 };
            }
            c if c.is_ascii_digit() || (c == '-' && next.is_some_and(|n| n.is_ascii_digit())) => {
                let start = i;
                i += 1;
                while i < chars.len() && chars[i].is_ascii_digit() {
                    i += 1;
                }
                let s: String = chars[start..i].iter().collect();
                out.push(Token::Num(s.parse().map_err(|_| format!("bad number '{s}'"))?));
            }
            c if c.is_ascii_alphabetic() || c == '_' => {
                let start = i;
                while i < chars.len()
                    && (chars[i].is_ascii_alphanumeric() || matches!(chars[i], '_' | ':' | '.' | '/'))
                {
                    i += 1;
                }
                let s: String = chars[start..i].iter().collect();
                out.push(Token::Ident(s.to_ascii_lowercase()));
            }
            c => return Err(format!("unexpected character '{c}'")),
        }
    }
    Ok(out)
}

struct MaskParser {
    tokens: Vec<Token>,
    at: usize,
}

impl MaskParser {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.at)
    }

    fn or(&mut self) -> Result<Mask, String> {
        let mut parts = vec![self.and()?];
        while self.peek() == Some(&Token::Or) {
            self.at += 1;
            parts.push(self.and()?);
        }
        Ok(if parts.len() == 1 { parts.pop().unwrap() } else { Mask::Or(parts) })
    }

    fn and(&mut self) -> Result<Mask, String> {
        let mut parts = vec![self.unary()?];
        while self.peek() == Some(&Token::And) {
            self.at += 1;
            parts.push(self.unary()?);
        }
        Ok(if parts.len() == 1 { parts.pop().unwrap() } else { Mask::And(parts) })
    }

    fn unary(&mut self) -> Result<Mask, String> {
        if self.peek() == Some(&Token::Not) {
            self.at += 1;
            return Ok(Mask::Not(Box::new(self.unary()?)));
        }
        self.atom()
    }

    fn atom(&mut self) -> Result<Mask, String> {
        let Some(tok) = self.tokens.get(self.at).cloned() else {
            return Err("expression ends early".into());
        };
        self.at += 1;
        match tok {
            Token::LParen => {
                let m = self.or()?;
                if self.peek() != Some(&Token::RParen) {
                    return Err("missing ')'".into());
                }
                self.at += 1;
                Ok(m)
            }
            Token::Ident(name) => match name.as_str() {
                "air" => Ok(Mask::Air),
                "solid" => Ok(Mask::Solid),
                "exposed" => Ok(Mask::Exposed),
                "surface" => Ok(Mask::Surface),
                "any" => Ok(Mask::Any),
                "y" => {
                    let Some(Token::Cmp(cmp)) = self.tokens.get(self.at).cloned() else {
                        return Err("'y' needs a comparison, like y>=10".into());
                    };
                    let Some(Token::Num(n)) = self.tokens.get(self.at + 1).cloned() else {
                        return Err("'y' comparison needs a number".into());
                    };
                    self.at += 2;
                    Ok(Mask::Y(cmp, n))
                }
                _ => Ok(Mask::Named(qualify(&name))),
            },
            t => Err(format!("unexpected {}", t.describe())),
        }
    }
}

// -------------------------------------------------------------- pattern

/// What to write: one block, or a weighted random mix.
#[derive(Clone, Debug, PartialEq)]
pub enum Pattern {
    Single(BlockId),
    Weighted(Vec<(BlockId, f32)>),
}

/// Parses `[N%]name, [N%]name, ...` into names and weights (default 1).
fn parse_entries(text: &str) -> Result<Vec<(String, f32)>, String> {
    let mut out = Vec::new();
    for part in text.split(',') {
        let part = part.trim();
        if part.is_empty() {
            return Err("empty entry".into());
        }
        let (weight, name) = match part.split_once('%') {
            Some((w, n)) => {
                let w: f32 = w.trim().parse().map_err(|_| format!("bad weight '{}'", w.trim()))?;
                if !(w > 0.0 && w.is_finite()) {
                    return Err("weights must be above 0".into());
                }
                (w, n.trim())
            }
            None => (1.0, part),
        };
        if name.is_empty()
            || !name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '.' | '/'))
        {
            return Err(format!("bad block name '{name}'"));
        }
        out.push((qualify(&name.to_ascii_lowercase()), weight));
    }
    Ok(out)
}

impl Pattern {
    /// Checks syntax without touching the block table.
    pub fn validate(text: &str) -> Result<(), String> {
        parse_entries(text).map(|_| ())
    }

    /// Parses and interns the named blocks.
    pub fn parse(text: &str, blocks: &mut BlockTable) -> Result<Pattern, String> {
        let entries = parse_entries(text)?;
        let mut items: Vec<(BlockId, f32)> =
            entries.into_iter().map(|(n, w)| (blocks.intern(BlockState::new(n)), w)).collect();
        Ok(if items.len() == 1 { Pattern::Single(items.pop().unwrap().0) } else { Pattern::Weighted(items) })
    }

    pub fn pick(&self, pos: IVec3, seed: u32) -> BlockId {
        match self {
            Pattern::Single(b) => *b,
            Pattern::Weighted(items) => {
                let total: f32 = items.iter().map(|(_, w)| w).sum();
                let mut roll = hash01(pos, seed) * total;
                for &(b, w) in items {
                    if roll < w {
                        return b;
                    }
                    roll -= w;
                }
                items.last().map_or(BlockId::AIR, |&(b, _)| b)
            }
        }
    }
}

// ---------------------------------------------------------------- brush

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BrushMode {
    /// Writes the pattern into every masked cell (a mask turns it into replace).
    Paint,
    /// Clears masked cells.
    Erase,
    /// Paint, but each cell only with probability `density`.
    Splatter { density: f32 },
    /// Puts the pattern on top of masked solid cells that have air above.
    Overlay,
    /// Fills and removes cells toward the majority of their 26 neighbours.
    Smooth,
    /// Removes solid cells with 3+ air face neighbours (corners and spikes).
    Erode,
    /// Fills air cells with 3+ solid face neighbours (notches and pits).
    Dilate,
}

#[derive(Clone, Debug)]
pub struct Brush {
    pub shape: Shape,
    pub mask: Mask,
    pub pattern: Pattern,
    pub mode: BrushMode,
    pub seed: u32,
}

impl Brush {
    /// Changes this brush would make with its shape centred on `center`.
    /// Only cells whose value actually changes are returned.
    pub fn plan(&self, world: &World, center: IVec3) -> Vec<(IVec3, BlockId)> {
        let mut out = Vec::new();
        let mut push = |p: IVec3, b: BlockId| {
            if world.get(p) != b {
                out.push((p, b));
            }
        };
        for p in self.shape.cells(center) {
            match self.mode {
                BrushMode::Paint => {
                    if self.mask.eval(world, p) {
                        push(p, self.pattern.pick(p, self.seed));
                    }
                }
                BrushMode::Erase => {
                    if self.mask.eval(world, p) {
                        push(p, BlockId::AIR);
                    }
                }
                BrushMode::Splatter { density } => {
                    if hash01(p, self.seed ^ 0x5bd1_e995) < density && self.mask.eval(world, p) {
                        push(p, self.pattern.pick(p, self.seed));
                    }
                }
                BrushMode::Overlay => {
                    let above = p + IVec3::Y;
                    if world.get(p) != BlockId::AIR
                        && world.get(above) == BlockId::AIR
                        && self.mask.eval(world, p)
                    {
                        push(above, self.pattern.pick(above, self.seed));
                    }
                }
                BrushMode::Smooth => {
                    if !self.mask.eval(world, p) {
                        continue;
                    }
                    let mut solid = Vec::new();
                    for dz in -1..=1 {
                        for dy in -1..=1 {
                            for dx in -1..=1 {
                                let b = world.get(p + IVec3::new(dx, dy, dz));
                                if (dx, dy, dz) != (0, 0, 0) && b != BlockId::AIR {
                                    solid.push(b);
                                }
                            }
                        }
                    }
                    let cur = world.get(p);
                    if solid.len() > 13 && cur == BlockId::AIR {
                        push(p, majority(&solid));
                    } else if solid.len() < 13 && cur != BlockId::AIR {
                        push(p, BlockId::AIR);
                    }
                }
                BrushMode::Erode => {
                    if world.get(p) != BlockId::AIR && self.mask.eval(world, p) {
                        let air = FACE_DIRS.iter().filter(|&&d| world.get(p + d) == BlockId::AIR);
                        if air.count() >= 3 {
                            push(p, BlockId::AIR);
                        }
                    }
                }
                BrushMode::Dilate => {
                    if world.get(p) == BlockId::AIR && self.mask.eval(world, p) {
                        let solid: Vec<BlockId> = FACE_DIRS
                            .iter()
                            .map(|&d| world.get(p + d))
                            .filter(|&b| b != BlockId::AIR)
                            .collect();
                        if solid.len() >= 3 {
                            push(p, majority(&solid));
                        }
                    }
                }
            }
        }
        out
    }
}

/// Most common block; ties go to the lowest id so results are stable.
fn majority(blocks: &[BlockId]) -> BlockId {
    let mut best = (BlockId::AIR, 0usize);
    for &b in blocks {
        let n = blocks.iter().filter(|&&o| o == b).count();
        if n > best.1 || (n == best.1 && b < best.0) {
            best = (b, n);
        }
    }
    best.0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world_with(names: &[&str]) -> (World, Vec<BlockId>) {
        let mut w = World::new();
        let ids = names.iter().map(|n| w.blocks.intern(BlockState::new(format!("minecraft:{n}")))).collect();
        (w, ids)
    }

    fn brush(shape: Shape, mode: BrushMode, pattern: BlockId) -> Brush {
        Brush { shape, mask: Mask::Any, pattern: Pattern::Single(pattern), mode, seed: 1 }
    }

    #[test]
    fn shape_cell_counts() {
        let c = IVec3::new(-3, 4, 9);
        assert_eq!(Shape::Sphere { radius: 0.0 }.cells(c), vec![c]);
        assert_eq!(Shape::Sphere { radius: 1.0 }.cells(c).len(), 7);
        assert_eq!(Shape::Cube { half: 1 }.cells(c).len(), 27);
        assert_eq!(Shape::Cylinder { radius: 1.0, half_height: 1 }.cells(c).len(), 15);
        assert!(Shape::Sphere { radius: 2.5 }.cells(c).contains(&(c + IVec3::new(2, 0, 0))));
    }

    #[test]
    fn mask_parsing_and_precedence() {
        assert_eq!(Mask::parse("  ").unwrap(), Mask::Any);
        assert_eq!(Mask::parse("stone").unwrap(), Mask::Named("minecraft:stone".into()));
        assert_eq!(Mask::parse("Mod:Thing").unwrap(), Mask::Named("mod:thing".into()));
        assert_eq!(Mask::parse("y>=-5").unwrap(), Mask::Y(Cmp::Ge, -5));
        // & binds tighter than |.
        assert_eq!(
            Mask::parse("air | solid & !exposed").unwrap(),
            Mask::Or(vec![
                Mask::Air,
                Mask::And(vec![Mask::Solid, Mask::Not(Box::new(Mask::Exposed))])
            ])
        );
        assert_eq!(Mask::parse("(air | solid) & y<3").unwrap(), Mask::And(vec![
            Mask::Or(vec![Mask::Air, Mask::Solid]),
            Mask::Y(Cmp::Lt, 3)
        ]));
    }

    #[test]
    fn mask_parse_errors() {
        for bad in ["(air", "air &", "y", "y>", "air solid", "stone )", "a $ b", "& air"] {
            assert!(Mask::parse(bad).is_err(), "{bad} should fail");
        }
    }

    #[test]
    fn mask_evaluation() {
        let (mut w, ids) = world_with(&["stone", "dirt"]);
        w.set(IVec3::new(0, 0, 0), ids[0]);
        w.set(IVec3::new(0, 1, 0), ids[1]);
        let at = |m: &str, p: IVec3| Mask::parse(m).unwrap().eval(&w, p);
        let bottom = IVec3::ZERO;
        let top = IVec3::new(0, 1, 0);
        assert!(at("stone", bottom) && !at("stone", top));
        assert!(at("dirt | stone", top));
        assert!(at("surface", top) && !at("surface", bottom));
        assert!(at("exposed", bottom)); // sides are air
        assert!(at("air", IVec3::new(5, 5, 5)) && at("!solid", IVec3::new(5, 5, 5)));
        assert!(at("y>=1 & dirt", top) && !at("y>=1", bottom));
    }

    #[test]
    fn unknown_names_are_reported() {
        let (w, _) = world_with(&["stone"]);
        let m = Mask::parse("stone | stne & !air").unwrap();
        assert_eq!(m.unknown_names(&w.blocks), vec!["minecraft:stne".to_string()]);
    }

    #[test]
    fn pattern_parsing() {
        let mut t = BlockTable::default();
        assert!(matches!(Pattern::parse("stone", &mut t), Ok(Pattern::Single(_))));
        let p = Pattern::parse("30%stone, 70% minecraft:dirt", &mut t).unwrap();
        assert!(matches!(&p, Pattern::Weighted(v) if v.len() == 2 && v[0].1 == 30.0));
        for bad in ["", "stone,", "x%stone", "0%stone", "-5%stone", "sto ne", "30%"] {
            assert!(Pattern::parse(bad, &mut t).is_err(), "{bad} should fail");
            assert!(Pattern::validate(bad).is_err());
        }
        assert!(Pattern::validate("a,b").is_ok());
    }

    #[test]
    fn weighted_pattern_follows_weights_and_is_deterministic() {
        let mut t = BlockTable::default();
        let p = Pattern::parse("25%stone,75%dirt", &mut t).unwrap();
        let stone = t.intern(BlockState::new("minecraft:stone"));
        let n = 20_000;
        let stones =
            (0..n).filter(|&i| p.pick(IVec3::new(i % 100, i / 100, 0), 7) == stone).count();
        let ratio = stones as f32 / n as f32;
        assert!((0.22..0.28).contains(&ratio), "ratio {ratio}");
        assert_eq!(p.pick(IVec3::new(3, 4, 5), 7), p.pick(IVec3::new(3, 4, 5), 7));
    }

    #[test]
    fn paint_and_replace_via_mask() {
        let (mut w, ids) = world_with(&["stone", "dirt", "glass"]);
        w.set(IVec3::new(0, 0, 0), ids[0]);
        w.set(IVec3::new(1, 0, 0), ids[1]);
        let mut b = brush(Shape::Cube { half: 1 }, BrushMode::Paint, ids[2]);
        assert_eq!(b.plan(&w, IVec3::ZERO).len(), 27);
        b.mask = Mask::parse("stone").unwrap();
        assert_eq!(b.plan(&w, IVec3::ZERO), vec![(IVec3::ZERO, ids[2])]);
    }

    #[test]
    fn paint_skips_cells_that_already_match() {
        let (mut w, ids) = world_with(&["stone"]);
        w.set(IVec3::ZERO, ids[0]);
        let b = brush(Shape::Sphere { radius: 0.0 }, BrushMode::Paint, ids[0]);
        assert!(b.plan(&w, IVec3::ZERO).is_empty());
    }

    #[test]
    fn erase_clears_only_masked_cells() {
        let (mut w, ids) = world_with(&["stone", "dirt"]);
        w.set(IVec3::new(0, 0, 0), ids[0]);
        w.set(IVec3::new(1, 0, 0), ids[1]);
        let mut b = brush(Shape::Cube { half: 1 }, BrushMode::Erase, ids[0]);
        b.mask = Mask::parse("dirt").unwrap();
        assert_eq!(b.plan(&w, IVec3::ZERO), vec![(IVec3::new(1, 0, 0), BlockId::AIR)]);
    }

    #[test]
    fn splatter_density_bounds() {
        let (w, ids) = world_with(&["stone"]);
        let shape = Shape::Cube { half: 8 }; // 4913 cells
        let none = brush(shape, BrushMode::Splatter { density: 0.0 }, ids[0]);
        let all = brush(shape, BrushMode::Splatter { density: 1.0 }, ids[0]);
        let half = brush(shape, BrushMode::Splatter { density: 0.5 }, ids[0]);
        assert!(none.plan(&w, IVec3::ZERO).is_empty());
        assert_eq!(all.plan(&w, IVec3::ZERO).len(), 4913);
        let n = half.plan(&w, IVec3::ZERO).len();
        assert!((2200..2700).contains(&n), "{n}");
        assert_eq!(n, half.plan(&w, IVec3::ZERO).len());
    }

    #[test]
    fn overlay_covers_the_top() {
        let (mut w, ids) = world_with(&["stone", "grass"]);
        for x in -2..=2 {
            for z in -2..=2 {
                w.set(IVec3::new(x, 0, z), ids[0]);
            }
        }
        let b = brush(Shape::Cube { half: 1 }, BrushMode::Overlay, ids[1]);
        let plan = b.plan(&w, IVec3::ZERO);
        assert_eq!(plan.len(), 9);
        assert!(plan.iter().all(|&(p, id)| p.y == 1 && id == ids[1]));
    }

    #[test]
    fn smooth_removes_a_spike_and_fills_a_pit() {
        let (mut w, ids) = world_with(&["stone"]);
        for x in -3..=3 {
            for z in -3..=3 {
                w.set(IVec3::new(x, 0, z), ids[0]);
                w.set(IVec3::new(x, -1, z), ids[0]);
            }
        }
        w.set(IVec3::new(0, 1, 0), ids[0]); // spike
        let b = brush(Shape::Cube { half: 2 }, BrushMode::Smooth, ids[0]);
        let plan = b.plan(&w, IVec3::ZERO);
        assert!(plan.contains(&(IVec3::new(0, 1, 0), BlockId::AIR)));
        // A flat floor is stable: nothing else on it changes.
        assert!(!plan.iter().any(|&(p, _)| p.y <= 0 && p.x.abs() <= 1 && p.z.abs() <= 1));

        w.set(IVec3::new(0, 1, 0), BlockId::AIR);
        w.set(IVec3::new(0, 0, 0), BlockId::AIR); // pit
        let plan = b.plan(&w, IVec3::ZERO);
        assert!(plan.contains(&(IVec3::ZERO, ids[0])));
    }

    #[test]
    fn erode_takes_corners_not_flats() {
        let (mut w, ids) = world_with(&["stone"]);
        for x in 0..=4 {
            for y in 0..=4 {
                for z in 0..=4 {
                    w.set(IVec3::new(x, y, z), ids[0]);
                }
            }
        }
        let b = brush(Shape::Cube { half: 4 }, BrushMode::Erode, ids[0]);
        let plan = b.plan(&w, IVec3::splat(2));
        assert_eq!(plan.len(), 8); // only the cube's 8 corners
        assert!(plan.iter().all(|&(_, id)| id == BlockId::AIR));
    }

    #[test]
    fn dilate_fills_a_notch_with_the_surrounding_block() {
        let (mut w, ids) = world_with(&["stone"]);
        for x in 0..=2 {
            for y in 0..=2 {
                for z in 0..=2 {
                    w.set(IVec3::new(x, y, z), ids[0]);
                }
            }
        }
        w.set(IVec3::ONE, BlockId::AIR); // sealed hole: 6 solid neighbours
        let b = brush(Shape::Sphere { radius: 0.0 }, BrushMode::Dilate, ids[0]);
        assert_eq!(b.plan(&w, IVec3::ONE), vec![(IVec3::ONE, ids[0])]);
        // A flat surface cell (one solid neighbour) is left alone.
        assert!(b.plan(&w, IVec3::new(1, 3, 1)).is_empty());
    }
}
