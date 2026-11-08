use crate::{models::StateModels, sprites::Sprites};
use horizon_assets::model::Direction;
use horizon_world::{chunk::BlockState, registry::{BlockRegistry, BlockStateId}};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs, path::Path,
};
const DEFAULT: u32 = u32::MAX;
const SKIP: u32 = u32::MAX - 1;
const HEADER: usize = 12;
#[derive(Default)]
pub struct ConnectedTextures {
    /// Relative word offsets; zero means keep the baked material.
    pub words: Vec<u32>,
    bindings: HashMap<(u32, u32, u8), u32>,
    /// Membership in the at most 32 distinct neighbor predicates in this milestone.
    /// Included in PT material interning, so different connectivity never collapses.
    pub neighbor_masks: Vec<u32>,
    pub loaded_rules: usize,
    pub skipped_rules: usize,
}
struct Rule {
    method: u32,
    width: u32,
    height: u32,
    tiles: Vec<u32>,
    weights: Vec<u32>,
    match_tiles: Vec<u32>,
    blocks: Vec<String>,
    faces: u8,
    symmetry: u32,
    loops: u32,
    linked: bool,
    connect: String,
}
impl Rule {
    fn matches(&self, state: &BlockState, layer: u32, face: u8) -> bool {
        self.faces & (1 << face) != 0
            && (self.match_tiles.is_empty() || self.match_tiles.contains(&layer))
            && (self.blocks.is_empty()
                || self.blocks.iter().any(|b| block_matches(b, state)))
    }
}
fn block_matches(pattern: &str, state: &BlockState) -> bool {
    let parts: Vec<_> = pattern.split(':').collect();
    let qualified = parts.len() > 1 && !parts[1].contains('=');
    let (name, from) = if qualified {
        (format!("{}:{}", parts[0], parts[1]), 2)
    } else {
        (format!("minecraft:{}", parts[0]), 1)
    };
    state.name == name
        && parts[from..]
            .iter()
            .all(|p| {
                p.split_once('=')
                    .is_some_and(|(key, values)| {
                        state
                            .properties
                            .get(key)
                            .is_some_and(|v| values.split(',').any(|x| x == v))
                    })
            })
}
/// Normalize the file-format's vanilla, namespace, absolute and rule-relative IDs.
fn tile_id(value: &str, ns: &str, dir: &str, matching: bool) -> Option<String> {
    let value = value.strip_suffix(".png").unwrap_or(value);
    let (namespace, path) = value.split_once(':').unwrap_or((ns, value));
    let path = if let Some(local) = path.strip_prefix("./") {
        format!("{dir}/{local}")
    } else if let Some(full) = path.strip_prefix("~/") {
        format!("optifine/{full}")
    } else if let Some(path) = path.strip_prefix("textures/") {
        path.to_owned()
    } else if path.starts_with("optifine/") || path.starts_with("block/") {
        path.to_owned()
    } else if matching {
        format!("block/{path}")
    } else if path.contains('/') {
        path.to_owned()
    } else {
        format!("{dir}/{path}")
    };
    let id = format!("{namespace}:{path}");
    crate::sprites::native_pt_assets::resource_path(Path::new(""), &id)?;
    Some(id)
}
fn tokens(text: &str) -> Vec<String> {
    text.split_whitespace()
        .flat_map(|value| {
            if let Some((a, b)) = value.split_once('-')
                && let (Ok(a), Ok(b)) = (a.parse::<u32>(), b.parse::<u32>()) && a <= b
                && b - a < 4096
            {
                return (a..=b).map(|n| n.to_string()).collect();
            }
            vec![value.to_owned()]
        })
        .collect()
}
fn load_rule(root: &Path, path: &Path, sprites: &mut Sprites) -> Option<(i32, Rule)> {
    let text = fs::read_to_string(path).ok()?;
    let props: BTreeMap<_, _> = text
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.starts_with(['#', '!']) {
                return None;
            }
            line.split_once('=').map(|(k, v)| (k.trim(), v.trim()))
        })
        .collect();
    if ["biomes", "heights", "minHeight", "maxHeight", "name"]
        .iter()
        .any(|k| props.contains_key(k))
    {
        return None;
    }
    let method = match *props.get("method")? {
        "fixed" => 0,
        "repeat" => 1,
        "random" => 2,
        "horizontal" => 3,
        "vertical" => 4,
        _ => return None,
    };
    let rel = path
        .strip_prefix(root.join("assets"))
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    let (ns, relative) = rel.split_once('/')?;
    let dir = relative.rsplit_once('/')?.0;
    let inferred = path.file_stem()?.to_str()?;
    let block_inferred = inferred.strip_prefix("block_");
    let blocks: Vec<String> = props
        .get("matchBlocks")
        .copied()
        .or((!props.contains_key("matchTiles")).then_some(block_inferred).flatten())
        .unwrap_or("")
        .split_whitespace()
        .map(str::to_owned)
        .collect();
    let match_tiles: Vec<u32> = props
        .get("matchTiles")
        .copied()
        .or_else(|| (blocks.is_empty()).then_some(inferred))
        .unwrap_or("")
        .split_whitespace()
        .filter_map(|s| {
            tile_id(s, ns, dir, true).and_then(|id| sprites.layers.get(&id).copied())
        })
        .collect();
    if blocks.is_empty() && match_tiles.is_empty()
        || props.contains_key("matchTiles") && match_tiles.is_empty()
    {
        return None;
    }
    let width = props.get("width").and_then(|s| s.parse().ok()).unwrap_or(1);
    let height = props.get("height").and_then(|s| s.parse().ok()).unwrap_or(1);
    let tile_names = tokens(props.get("tiles")?);
    if tile_names.is_empty() || tile_names.len() > 4096
        || (method == 1
            && (width == 0 || height == 0 || width > 64 || height > 64
                || width * height != tile_names.len() as u32))
        || (matches!(method, 3 | 4) && tile_names.len() != 4)
        || (method == 0 && tile_names.len() != 1)
    {
        return None;
    }
    let mut tiles = Vec::new();
    for name in tile_names {
        tiles
            .push(
                match name.as_str() {
                    "<default>" => DEFAULT,
                    "<skip>" => SKIP,
                    _ => {
                        let id = tile_id(&name, ns, dir, false)?;
                        if !crate::sprites::native_pt_assets::resource_path(root, &id)?
                            .is_file()
                        {
                            return None;
                        }
                        sprites.insert(root, &id).ok()?
                    }
                },
            );
    }
    let mut weights: Vec<u32> = props
        .get("weights")
        .map(|s| tokens(s).iter().filter_map(|s| s.parse().ok()).collect())
        .unwrap_or_default();
    let average = weights
        .iter()
        .copied()
        .map(u64::from)
        .sum::<u64>()
        .checked_div(weights.len() as u64)
        .unwrap_or(1)
        .min(u64::from(u32::MAX)) as u32;
    weights.truncate(tiles.len());
    weights.resize(tiles.len(), average);
    if weights.iter().copied().map(u64::from).sum::<u64>() > u64::from(u32::MAX) {
        return None;
    }
    if weights.iter().all(|&w| w == 0) {
        return None;
    }
    let mut faces = 0;
    for face in props.get("faces").unwrap_or(&"all").split_whitespace() {
        faces
            |= match face {
                "all" => 63,
                "sides" => 60,
                "bottom" | "down" => 1,
                "top" | "up" => 2,
                "north" => 4,
                "south" => 8,
                "west" => 16,
                "east" => 32,
                _ => 0,
            };
    }
    let weight = props.get("weight").and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
    Some((
        weight,
        Rule {
            method,
            width,
            height,
            tiles,
            weights,
            match_tiles,
            blocks,
            faces,
            symmetry: match props.get("symmetry").copied().unwrap_or("none") {
                "opposite" => 1,
                "all" => 2,
                _ => 0,
            },
            loops: props
                .get("randomLoops")
                .and_then(|s| s.parse::<u32>().ok())
                .unwrap_or(0)
                .min(9),
            linked: props.get("linked") == Some(&"true"),
            connect: props
                .get("connect")
                .copied()
                .unwrap_or(
                    if props.contains_key("matchTiles") { "tile" } else { "block" },
                )
                .to_owned(),
        },
    ))
}
impl ConnectedTextures {
    /// Conservative material set for emitter discovery, including chained tiles.
    /// The GPU still evaluates the actual position-dependent CTM selection.
    pub(crate) fn possible_layers(&self, program: u32, baked: u32) -> BTreeSet<u32> {
        let mut layers = BTreeSet::from([baked]);
        let mut visited = BTreeSet::new();
        let mut pending = vec![program];
        while let Some(node) = pending.pop() {
            let h = node as usize;
            if node == 0 || !visited.insert(node) || h + HEADER > self.words.len() {
                continue;
            }
            let count = self.words[h + 3] as usize;
            let start = self.words[h + 10] as usize;
            let Some(tiles) = self.words.get(start..start.saturating_add(count * 3))
            else {
                continue;
            };
            for tile in tiles.chunks(3) {
                if tile[0] < SKIP {
                    layers.insert(tile[0]);
                }
                pending.push(tile[1]);
            }
        }
        layers
    }
    pub fn load(
        root: &Path,
        registry: &BlockRegistry,
        models: &StateModels,
        sprites: &mut Sprites,
    ) -> Self {
        let mut paths = Vec::new();
        let namespaces = fs::read_dir(root.join("assets"))
            .into_iter()
            .flatten()
            .flatten();
        for ns in namespaces {
            let mut stack = vec![ns.path().join("optifine/ctm")];
            while let Some(dir) = stack.pop() {
                for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
                    let path = entry.path();
                    if path.is_dir() {
                        stack.push(path);
                    } else if path.extension().is_some_and(|p| p == "properties") {
                        paths.push(path);
                    }
                }
            }
        }
        paths.sort();
        if paths.is_empty() {
            return Self::default();
        }
        for path in &paths {
            let Ok(text) = fs::read_to_string(path) else {
                continue;
            };
            let rel = path
                .strip_prefix(root.join("assets"))
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            let (ns, rel) = rel.split_once('/').unwrap();
            let dir = rel.rsplit_once('/').unwrap().0;
            let supported = text
                .lines()
                .any(|l| {
                    l.trim()
                        .split_once('=')
                        .is_some_and(|(k, v)| {
                            k.trim() == "method"
                                && matches!(
                                    v.trim(), "repeat" | "random" | "fixed" | "horizontal" |
                                    "vertical"
                                )
                        })
                });
            if !supported {
                continue;
            }
            for line in text.lines() {
                if let Some((k, v)) = line.trim().split_once('=') && k.trim() == "tiles"
                {
                    for tile in tokens(v) {
                        if tile.starts_with('<') {
                            continue;
                        }
                        if let Some(id) = tile_id(&tile, ns, dir, false)
                            && crate::sprites::native_pt_assets::resource_path(root, &id)
                                .is_some_and(|p| p.is_file())
                        {
                            let _ = sprites.insert(root, &id);
                        }
                    }
                }
            }
        }
        let mut rules: Vec<_> = paths
            .iter()
            .filter_map(|p| {
                load_rule(root, p, sprites)
                    .map(|(weight, rule)| (p.clone(), weight, rule))
            })
            .collect();
        rules
            .sort_by(|a, b| {
                a.2
                    .match_tiles
                    .is_empty()
                    .cmp(&b.2.match_tiles.is_empty())
                    .then_with(|| b.1.cmp(&a.1))
                    .then_with(|| a.0.cmp(&b.0))
            });
        let skipped_rules = paths.len() - rules.len();
        let rules: Vec<_> = rules.into_iter().map(|(_, _, r)| r).collect();
        let mut tile_rules: HashMap<u32, Vec<usize>> = HashMap::new();
        let mut block_rules: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, rule) in rules.iter().enumerate() {
            for &layer in &rule.match_tiles {
                tile_rules.entry(layer).or_default().push(i);
            }
            if rule.match_tiles.is_empty() {
                for pattern in &rule.blocks {
                    let parts: Vec<_> = pattern.split(':').collect();
                    let name = if parts.len() > 1 && !parts[1].contains('=') {
                        format!("{}:{}", parts[0], parts[1])
                    } else {
                        format!("minecraft:{}", parts[0])
                    };
                    block_rules.entry(name).or_default().push(i);
                }
            }
        }
        let keys: Vec<_> = (0..registry.len())
            .map(|i| models.face_keys(BlockStateId(i as u32)))
            .collect();
        let mut builder = Builder {
            result: Self {
                words: vec![0],
                neighbor_masks: vec![0; registry.len()],
                loaded_rules: rules.len(),
                skipped_rules,
                ..Default::default()
            },
            rules: &rules,
            registry,
            keys: &keys,
            nodes: HashMap::new(),
            predicates: BTreeMap::new(),
            tile_rules,
            block_rules,
            connections: HashMap::new(),
        };
        for (state, faces) in keys.iter().enumerate() {
            for &(layer, face) in faces {
                let node = builder.compile(state as u32, layer, face, 0, &[]);
                if node != 0 {
                    builder.result.bindings.insert((state as u32, layer, face), node);
                }
            }
        }
        log::info!(
            "CTM: {} rules, {} skipped, {} bound faces, {} neighbor predicates, {} words",
            rules.len(), skipped_rules, builder.result.bindings.len(), builder.predicates
            .len(), builder.result.words.len()
        );
        builder.result
    }
    pub fn program(&self, state: BlockStateId, layer: u32, face: Direction) -> u32 {
        self.bindings.get(&(state.0, layer, face as u8)).copied().unwrap_or(0)
    }
    pub fn select(
        &self,
        state: BlockStateId,
        layer: u32,
        face: Direction,
        pos: [i32; 3],
        neighbor: impl Fn([i32; 3]) -> u32,
    ) -> u32 {
        self.evaluate(
            self.program(state, layer, face),
            layer,
            pos,
            |delta| {
                self.neighbor_masks.get(neighbor(delta) as usize).copied().unwrap_or(0)
            },
        )
    }
    pub fn evaluate(
        &self,
        mut node: u32,
        mut layer: u32,
        pos: [i32; 3],
        neighbor_mask: impl Fn([i32; 3]) -> u32,
    ) -> u32 {
        for _ in 0..8 {
            if node == 0 {
                break;
            }
            let h = &self.words[node as usize..node as usize + HEADER];
            let face = h[4];
            let mut index = 0;
            if h[0] == 1 {
                let [u, v] = plane(pos, face, h[5]);
                index = v.rem_euclid(h[2] as i32) as u32 * h[1]
                    + u.rem_euclid(h[1] as i32) as u32;
            } else if h[0] == 2 {
                let mut p = pos;
                p[1] -= h[7] as i32;
                let f = match h[5] {
                    1 => face & !1,
                    2 => 0,
                    _ => face,
                };
                let mut random = (p[0] as u32).wrapping_mul(73428767)
                    ^ (p[1] as u32).wrapping_mul(912931)
                    ^ (p[2] as u32).wrapping_mul(438289) ^ f.wrapping_mul(19937);
                for _ in 0..=h[6] {
                    random = hash(random);
                }
                let pick = random % h[9];
                index = (0..h[3])
                    .find(|&i| pick < self.words[h[10] as usize + i as usize * 3 + 2])
                    .unwrap_or(0);
            } else if matches!(h[0], 3 | 4) {
                let delta = if h[0] == 4 { [0, 1, 0] } else { horizontal(face) };
                let positive = neighbor_mask(delta) & h[8] != 0;
                let negative = neighbor_mask(delta.map(|x| -x)) & h[8] != 0;
                index = match (negative, positive) {
                    (false, true) => 0,
                    (true, true) => 1,
                    (true, false) => 2,
                    _ => 3,
                };
                if h[0] == 4 && index != 1 && index != 3 {
                    index = 2 - index;
                }
            }
            let tile = h[10] as usize + index as usize * 3;
            if self.words[tile] < SKIP {
                layer = self.words[tile];
            }
            node = self.words[tile + 1];
        }
        layer
    }
}
struct Builder<'a> {
    result: ConnectedTextures,
    rules: &'a [Rule],
    registry: &'a BlockRegistry,
    keys: &'a [BTreeSet<(u32, u8)>],
    nodes: HashMap<Vec<u32>, u32>,
    predicates: BTreeMap<Vec<u32>, u32>,
    tile_rules: HashMap<u32, Vec<usize>>,
    block_rules: HashMap<String, Vec<usize>>,
    connections: HashMap<(usize, u32, u8, String), u32>,
}
impl Builder<'_> {
    fn compile(
        &mut self,
        state_id: u32,
        layer: u32,
        face: u8,
        start: usize,
        visited: &[usize],
    ) -> u32 {
        if visited.len() >= 8 {
            return 0;
        }
        let state = self.registry.state(BlockStateId(state_id)).unwrap();
        let candidates = self
            .tile_rules
            .get(&layer)
            .into_iter()
            .flatten()
            .chain(self.block_rules.get(&state.name).into_iter().flatten());
        let Some(i) = candidates
            .copied()
            .find(|&i| {
                i >= start && !visited.contains(&i)
                    && self.rules[i].matches(state, layer, face)
            }) else {
            return 0;
        };
        let rule = &self.rules[i];
        let mut history = visited.to_vec();
        history.push(i);
        let mut mask = 0;
        if matches!(rule.method, 3 | 4) {
            let identity = match rule.connect.as_str() {
                "block" => state.name.clone(),
                "state" => state_id.to_string(),
                _ => String::new(),
            };
            let key = (i, layer, face, identity);
            if let Some(&known) = self.connections.get(&key) {
                mask = known;
            } else {
                let members: Vec<u32> = (0..self.registry.len())
                    .filter(|&id| {
                        let other = self
                            .registry
                            .state(BlockStateId(id as u32))
                            .unwrap();
                        match rule.connect.as_str() {
                            "block" => other.name == state.name,
                            "state" => id == state_id as usize,
                            _ => self.keys[id].contains(&(layer, face)),
                        }
                    })
                    .map(|i| i as u32)
                    .collect();
                mask = if let Some(mask) = self.predicates.get(&members) {
                    *mask
                } else {
                    if self.predicates.len() >= 32 {
                        log::warn!(
                            "CTM neighbor predicate budget reached; leaving this face unchanged"
                        );
                        return 0;
                    }
                    let bit = 1u32 << self.predicates.len();
                    for &id in &members {
                        self.result.neighbor_masks[id as usize] |= bit;
                    }
                    self.predicates.insert(members, bit);
                    bit
                };
                self.connections.insert(key, mask);
            }
        }
        let upper = rule.linked
            && state.properties.get("half").is_some_and(|s| s == "upper");
        let mut node = vec![
            rule.method, rule.width, rule.height, rule.tiles.len() as u32,
            u32::from(face), rule.symmetry, rule.loops, u32::from(upper), mask, rule
            .weights.iter().sum(), 0, 0,
        ];
        let mut cumulative = 0;
        for (j, &tile) in rule.tiles.iter().enumerate() {
            let next = match tile {
                DEFAULT => 0,
                SKIP => self.compile(state_id, layer, face, i + 1, &history),
                _ if tile == layer => 0,
                _ => self.compile(state_id, tile, face, 0, &history),
            };
            cumulative += rule.weights[j];
            node.extend([tile, next, cumulative]);
        }
        if let Some(&offset) = self.nodes.get(&node) {
            return offset;
        }
        let offset = self.result.words.len() as u32;
        let key = node.clone();
        node[10] = offset + HEADER as u32;
        self.result.words.extend(node);
        self.nodes.insert(key, offset);
        offset
    }
}
fn hash(mut n: u32) -> u32 {
    n = (n ^ (n >> 16)).wrapping_mul(0x7feb352d);
    n = (n ^ (n >> 15)).wrapping_mul(0x846ca68b);
    n ^ (n >> 16)
}
fn plane([x, y, z]: [i32; 3], face: u32, symmetry: u32) -> [i32; 2] {
    match face {
        0 => [x, -z - 1],
        1 => [x, z],
        2 => [-x - 1, -y - 1],
        3 => [if symmetry == 1 { -x - 1 } else { x }, -y - 1],
        4 => [z, -y - 1],
        _ => [if symmetry == 1 { z } else { -z - 1 }, -y - 1],
    }
}
fn horizontal(face: u32) -> [i32; 3] {
    match face {
        2 => [-1, 0, 0],
        3 => [1, 0, 0],
        4 => [0, 0, 1],
        5 => [0, 0, -1],
        _ => [1, 0, 0],
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn program(
        method: u32,
        face: Direction,
        width: u32,
        height: u32,
    ) -> ConnectedTextures {
        let count = match method {
            1 => width * height,
            3 | 4 => 4,
            _ => 3,
        };
        let mut words = vec![
            0, method, width, height, count, face as u32, 0, 0, 0, 1, count, 13, 0,
        ];
        for i in 0..count {
            words.extend([10 + i, 0, i + 1]);
        }
        ConnectedTextures {
            words,
            ..Default::default()
        }
    }
    #[test]
    fn repeats_are_continuous_on_six_faces_across_negative_chunk_boundaries() {
        for face in Direction::ALL {
            let ctm = program(1, face, 4, 4);
            let mut selected = BTreeSet::new();
            for a in -18..18 {
                for b in -18..18 {
                    let p = match face {
                        Direction::Up | Direction::Down => [a, 7, b],
                        Direction::North | Direction::South => [a, b, 7],
                        _ => [7, b, a],
                    };
                    let layer = ctm.evaluate(1, 0, p, |_| 0);
                    selected.insert(layer);
                    let shift = match face {
                        Direction::Up | Direction::Down => [4, 0, 4],
                        Direction::North | Direction::South => [4, 4, 0],
                        _ => [0, 4, 4],
                    };
                    assert_eq!(
                        layer, ctm.evaluate(1, 0, std::array::from_fn(| i | p[i] +
                        shift[i]), | _ | 0)
                    );
                    let [u, v] = plane(p, face as u32, 0);
                    assert_eq!(
                        layer, 10 + (v.rem_euclid(4) * 4 + u.rem_euclid(4)) as u32
                    );
                }
            }
            assert_eq!(selected.len(), 16);
        }
        assert_eq!(plane([15, 0, 0], 3, 0), [15, - 1]);
        assert_eq!(plane([16, 0, 0], 3, 0), [16, - 1]);
        assert_eq!(plane([- 1, 0, 0], 2, 0), [0, - 1]);
        assert_eq!(plane([0, 0, 0], 2, 0), [- 1, - 1]);
    }
    #[test]
    fn horizontal_and_vertical_tiles_respond_to_both_neighbors() {
        for face in Direction::ALL {
            for method in [3, 4] {
                let ctm = program(method, face, 1, 1);
                let positive = if method == 4 {
                    [0, 1, 0]
                } else {
                    horizontal(face as u32)
                };
                for (a, b, index) in [
                    (false, false, 3),
                    (true, false, 0),
                    (false, true, 2),
                    (true, true, 1),
                ] {
                    let expected = if method == 4 && index != 1 && index != 3 {
                        2 - index
                    } else {
                        index
                    };
                    assert_eq!(
                        ctm.evaluate(1, 0, [16, 70, - 16], | d | u32::from(d == positive
                        && a || d == positive.map(| x | - x) && b)), 10 + expected
                    );
                }
            }
        }
    }
    #[test]
    fn weighted_random_is_stable_and_symmetry_links_faces_and_upper_halves() {
        let mut ctm = program(2, Direction::North, 1, 1);
        ctm.words[10] = 6;
        ctm.words[15] = 1;
        ctm.words[18] = 4;
        ctm.words[21] = 6;
        let values: Vec<_> = (-100..100)
            .map(|x| ctm.evaluate(1, 0, [x, 70, -17], |_| 0))
            .collect();
        assert!(values.iter().copied().collect::< BTreeSet < _ >> ().len() > 1);
        for (i, x) in (-100..100).enumerate() {
            assert_eq!(values[i], ctm.evaluate(1, 0, [x, 70, - 17], | _ | 0));
        }
        ctm.words[6] = 1;
        let north = ctm.evaluate(1, 0, [-17, 70, 16], |_| 0);
        ctm.words[5] = Direction::South as u32;
        assert_eq!(north, ctm.evaluate(1, 0, [- 17, 70, 16], | _ | 0));
        ctm.words[8] = 1;
        assert_eq!(north, ctm.evaluate(1, 0, [- 17, 71, 16], | _ | 0));
    }
    #[test]
    fn block_properties_paths_and_token_ranges_follow_the_format() {
        let state = BlockState {
            name: "minecraft:oak_log".into(),
            properties: BTreeMap::from([("axis".into(), "x".into())]),
        };
        assert!(block_matches("minecraft:oak_log:axis=x,z", & state));
        assert!(block_matches("oak_log:axis=x", & state));
        assert!(! block_matches("oak_log:axis=y", & state));
        assert!(! block_matches("oak_wood", & state));
        assert_eq!(tokens("1-3 foo <default>"), ["1", "2", "3", "foo", "<default>"]);
        assert_eq!(
            tile_id("./1.png", "minecraft", "optifine/ctm/a", true).unwrap(),
            "minecraft:optifine/ctm/a/1"
        );
        assert_eq!(
            tile_id("textures/block/extra/a.png", "minecraft", "x", true).unwrap(),
            "minecraft:block/extra/a"
        );
        assert!(tile_id("../bad", "minecraft", "x", false).is_none());
    }
    #[test]
    fn compiler_chains_selected_tiles_and_does_not_loop_or_misapply_states() {
        let facts = json!(
            { "states" : [["", false, false, "", { "id" : "minecraft:oak_log",
            "properties" : { "axis" : "x" } }], ["", false, false, "", { "id" :
            "minecraft:oak_log", "properties" : { "axis" : "y" } }]], "defaults" : {
            "minecraft:oak_log" : 0 } }
        );
        let registry = BlockRegistry::from_facts_json(facts.to_string().as_bytes())
            .unwrap();
        let rule = |input, output, blocks: Vec<String>| Rule {
            method: 0,
            width: 1,
            height: 1,
            tiles: vec![output],
            weights: vec![1],
            match_tiles: vec![input],
            blocks,
            faces: 63,
            symmetry: 0,
            loops: 0,
            linked: false,
            connect: "tile".into(),
        };
        let rules = vec![
            rule(1, 2, vec!["oak_log:axis=x".into()]), rule(2, 3, vec![]), rule(3, 1,
            vec![]),
        ];
        let keys = vec![BTreeSet::from([(1, 2)]); 2];
        let mut builder = Builder {
            result: ConnectedTextures {
                words: vec![0],
                neighbor_masks: vec![0; 2],
                ..Default::default()
            },
            rules: &rules,
            registry: &registry,
            keys: &keys,
            nodes: HashMap::new(),
            predicates: BTreeMap::new(),
            tile_rules: HashMap::from([(1, vec![0]), (2, vec![1]), (3, vec![2])]),
            block_rules: HashMap::new(),
            connections: HashMap::new(),
        };
        let offset = builder.compile(0, 1, 2, 0, &[]);
        assert_ne!(offset, 0);
        assert_eq!(builder.compile(1, 1, 2, 0, & []), 0);
        assert_eq!(builder.result.evaluate(offset, 1, [0, 0, 0], | _ | 0), 1);
        builder.rules = &rules[..2];
        builder.tile_rules.remove(&3);
        builder.nodes.clear();
        let offset = builder.compile(0, 1, 2, 0, &[]);
        assert_eq!(builder.result.evaluate(offset, 1, [0, 0, 0], | _ | 0), 3);
    }
    #[test]
    fn gpu_tile_selection_matches_cpu_for_all_faces_methods_and_neighbors() {
        use wgpu::util::DeviceExt;
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN,
            ..wgpu::InstanceDescriptor::new_without_display_handle()
        });
        let adapter = pollster::block_on(instance.request_adapter(&Default::default()))
            .unwrap();
        let (device, queue) = pollster::block_on(
                adapter
                    .request_device(
                        &wgpu::DeviceDescriptor {
                            required_limits: adapter.limits(),
                            ..Default::default()
                        },
                    ),
            )
            .unwrap();
        let mut packed = vec![0, 0, 0, 4];
        let mut cases = Vec::new();
        let mut expected = Vec::new();
        let mut voxels = vec![0u32; 128 * 128 * 128];
        let origin = [-64, -64, -64];
        let index = |p: [i32; 3]| (p[2] * 128 * 128 + p[1] * 128 + p[0]) as usize;
        for face in Direction::ALL {
            for method in 0..=4 {
                for context in 0..4 {
                    let n = cases.len();
                    let p = [
                        8 + (n % 10) as i32 * 10,
                        8 + (n / 10 % 10) as i32 * 10,
                        8 + (n / 100) as i32 * 10,
                    ];
                    let ctm = program(method, face, 4, 4);
                    let delta = if method == 4 {
                        [0, 1, 0]
                    } else {
                        horizontal(face as u32)
                    };
                    for sign in [-1, 1] {
                        if context & if sign == 1 { 1 } else { 2 } != 0 {
                            voxels[index(
                                std::array::from_fn(|i| p[i] + delta[i] * sign),
                            )] = 1;
                        }
                    }
                    let world = std::array::from_fn(|i| p[i] + origin[i]);
                    expected
                        .push(
                            ctm
                                .evaluate(
                                    1,
                                    5,
                                    world,
                                    |d| { voxels[index(std::array::from_fn(|i| p[i] + d[i]))] },
                                ),
                        );
                    let offset = packed.len() as u32 - 4;
                    let mut words = ctm.words;
                    words[11] += offset;
                    cases
                        .push([
                            p[0] as u32,
                            p[1] as u32,
                            p[2] as u32,
                            0,
                            offset + 1,
                            5,
                            0,
                            0,
                        ]);
                    packed.extend(words);
                }
            }
        }
        let source = format!(
            "
        struct Frame {{ origin:vec4<i32> }}
        struct Triangle {{ e2:vec4<f32>, uv2_layer:vec4<f32> }}
        struct Material {{ range:vec4<u32> }}
        struct Input {{ p:vec4<i32>, data:vec4<u32> }}
        @group(0) @binding(0) var<storage,read> pbr_data:array<u32>;
        @group(0) @binding(1) var blocks:texture_3d<u32>;
        @group(0) @binding(2) var<storage,read> materials:array<Material>;
        @group(0) @binding(3) var<storage,read> inputs:array<Input>;
        @group(0) @binding(4) var<storage,read_write> results:array<u32>;
        @group(0) @binding(5) var<uniform> frame:Frame;
        {}
        @compute @workgroup_size(1)
        fn main(@builtin(global_invocation_id) id:vec3<u32>) {{
            var tri:Triangle;
            tri.e2.w=f32(inputs[id.x].data.x);
            tri.uv2_layer.z=f32(inputs[id.x].data.y);
            results[id.x]=ctm_layer(tri,inputs[id.x].p.xyz);
        }}",
            "// Shared packed program format: ctm.rs. Absolute block coordinates keep tiling\r\n// anchored across camera-relative voxel-cache shifts and negative chunks.\r\nfn ctm_hash(value: u32) -> u32 {\r\n    var n=(value^(value>>16u))*0x7feb352du;\r\n    n=(n^(n>>15u))*0x846ca68bu;\r\n    return n^(n>>16u);\r\n}\r\nfn ctm_mod(n: i32, size: u32) -> u32 {\r\n    let s=i32(size);\r\n    return u32(((n%s)+s)%s);\r\n}\r\nfn ctm_plane(p: vec3<i32>, face: u32, symmetry: u32) -> vec2<i32> {\r\n    switch face {\r\n        case 0u: { return vec2<i32>(p.x,-p.z-1); }\r\n        case 1u: { return p.xz; }\r\n        case 2u: { return vec2<i32>(-p.x-1,-p.y-1); }\r\n        case 3u: { return vec2<i32>(select(p.x,-p.x-1,symmetry==1u),-p.y-1); }\r\n        case 4u: { return vec2<i32>(p.z,-p.y-1); }\r\n        default: { return vec2<i32>(select(-p.z-1,p.z,symmetry==1u),-p.y-1); }\r\n    }\r\n}\r\nfn ctm_horizontal(face: u32) -> vec3<i32> {\r\n    switch face {\r\n        case 2u: { return vec3<i32>(-1,0,0); }\r\n        case 3u: { return vec3<i32>(1,0,0); }\r\n        case 4u: { return vec3<i32>(0,0,1); }\r\n        case 5u: { return vec3<i32>(0,0,-1); }\r\n        default: { return vec3<i32>(1,0,0); }\r\n    }\r\n}\r\nfn ctm_neighbor(cell: vec3<i32>, mask: u32) -> bool {\r\n    if any(cell<vec3<i32>(0)) || any(cell>=vec3<i32>(128)) { return false; }\r\n    let state=textureLoad(blocks,cell,0).x;\r\n    if state>=arrayLength(&materials) { return false; }\r\n    return (materials[state].range.w&mask)!=0u;\r\n}\r\nfn ctm_layer(tri: Triangle, cell: vec3<i32>) -> u32 {\r\n    var layer=u32(tri.uv2_layer.z);\r\n    var node=u32(tri.e2.w);\r\n    if node==0u { return layer; }\r\n    let base=pbr_data[3];\r\n    let world=cell+frame.origin.xyz;\r\n    for(var depth=0u; depth<8u; depth++) {\r\n        if node==0u || base+node+11u>=arrayLength(&pbr_data) { break; }\r\n        let h=base+node;\r\n        let method=pbr_data[h];\r\n        let face=pbr_data[h+4u];\r\n        let symmetry=pbr_data[h+5u];\r\n        var index=0u;\r\n        if method==1u {\r\n            let uv=ctm_plane(world,face,symmetry);\r\n            index=ctm_mod(uv.y,pbr_data[h+2u])*pbr_data[h+1u]+ctm_mod(uv.x,pbr_data[h+1u]);\r\n        } else if method==2u {\r\n            let p=world-vec3<i32>(0,i32(pbr_data[h+7u]),0);\r\n            var f=face;\r\n            if symmetry==1u { f=face&0xfffffffeu; }\r\n            if symmetry==2u { f=0u; }\r\n            var random=u32(p.x)*73428767u ^ u32(p.y)*912931u ^ u32(p.z)*438289u ^ f*19937u;\r\n            for(var iteration=0u; iteration<=pbr_data[h+6u]; iteration++) { random=ctm_hash(random); }\r\n            let pick=random%pbr_data[h+9u];\r\n            for(var tile=0u; tile<pbr_data[h+3u]; tile++) {\r\n                if pick<pbr_data[base+pbr_data[h+10u]+tile*3u+2u] { index=tile; break; }\r\n            }\r\n        } else if method==3u || method==4u {\r\n            var delta=ctm_horizontal(face);\r\n            if method==4u { delta=vec3<i32>(0,1,0); }\r\n            let positive=ctm_neighbor(cell+delta,pbr_data[h+8u]);\r\n            let negative=ctm_neighbor(cell-delta,pbr_data[h+8u]);\r\n            index=3u;\r\n            if positive { index=0u; }\r\n            if negative { index=2u; }\r\n            if positive && negative { index=1u; }\r\n            if method==4u && (index==0u || index==2u) { index=2u-index; }\r\n        }\r\n        let tile=base+pbr_data[h+10u]+index*3u;\r\n        let chosen=pbr_data[tile];\r\n        if chosen<0xfffffffeu { layer=chosen; }\r\n        node=pbr_data[tile+1u];\r\n    }\r\n    return layer;\r\n}\r\n"
        );
        let module = device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("CTM CPU/GPU parity"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
        let pipeline = device
            .create_compute_pipeline(
                &wgpu::ComputePipelineDescriptor {
                    label: None,
                    layout: None,
                    module: &module,
                    entry_point: Some("main"),
                    compilation_options: Default::default(),
                    cache: None,
                },
            );
        let storage = |data: &[u8]| {
            device
                .create_buffer_init(
                    &wgpu::util::BufferInitDescriptor {
                        label: None,
                        contents: data,
                        usage: wgpu::BufferUsages::STORAGE,
                    },
                )
        };
        let data = storage(bytemuck::cast_slice(&packed));
        let inputs = storage(bytemuck::cast_slice(&cases));
        let materials = storage(bytemuck::cast_slice(&[0u32, 0, 0, 0, 0, 0, 0, 1]));
        let frame = device
            .create_buffer_init(
                &wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(&[-64i32, -64, -64, 0]),
                    usage: wgpu::BufferUsages::UNIFORM,
                },
            );
        let output = device
            .create_buffer(
                &wgpu::BufferDescriptor {
                    label: None,
                    size: expected.len() as u64 * 4,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                },
            );
        let read = device
            .create_buffer(
                &wgpu::BufferDescriptor {
                    label: None,
                    size: output.size(),
                    usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                },
            );
        let texture = device
            .create_texture(
                &wgpu::TextureDescriptor {
                    label: None,
                    size: wgpu::Extent3d {
                        width: 128,
                        height: 128,
                        depth_or_array_layers: 128,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D3,
                    format: wgpu::TextureFormat::R32Uint,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                },
            );
        queue
            .write_texture(
                texture.as_image_copy(),
                bytemuck::cast_slice(&voxels),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(512),
                    rows_per_image: Some(128),
                },
                texture.size(),
            );
        let view = texture.create_view(&Default::default());
        let resources = [
            data.as_entire_binding(),
            wgpu::BindingResource::TextureView(&view),
            materials.as_entire_binding(),
            inputs.as_entire_binding(),
            output.as_entire_binding(),
            frame.as_entire_binding(),
        ];
        let entries: Vec<_> = resources
            .into_iter()
            .enumerate()
            .map(|(i, resource)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource,
            })
            .collect();
        let group = device
            .create_bind_group(
                &wgpu::BindGroupDescriptor {
                    label: None,
                    layout: &pipeline.get_bind_group_layout(0),
                    entries: &entries,
                },
            );
        let mut encoder = device.create_command_encoder(&Default::default());
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &group, &[]);
            pass.dispatch_workgroups(cases.len() as u32, 1, 1);
        }
        encoder.copy_buffer_to_buffer(&output, 0, &read, 0, output.size());
        queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        read.slice(..)
            .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        rx.recv().unwrap().unwrap();
        let bytes = read.slice(..).get_mapped_range().unwrap();
        let actual: &[u32] = bytemuck::cast_slice(&bytes);
        assert_eq!(actual, expected, "raster CPU and traced GPU tiles must agree");
    }
}
