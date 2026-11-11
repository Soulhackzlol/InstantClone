//! A coding unit's residual as a transform tree (H.265 7.3.8.8 to
//! 7.3.8.10): one transform block the size of the unit, or the unit split
//! evenly into four, sixteen or sixty-four, down to 4x4.
//!
//! Small detail (a spark, a dot) packs into a few levels of a 4x4 or 8x8
//! transform but spreads over most of a 16x16 one, so units try the splits
//! and keep the cheapest. A carried-over unit's prediction is the same
//! whichever split, so its residual is transformed once per split. An
//! intra unit's blocks each predict from the ones decoded before them, its
//! own earlier blocks included, so a split intra unit is coded block by
//! block, as a decoder rebuilds it, at each QP it tries (and 4x4 luma
//! blocks take the DST there).
//!
//! In 4:2:0 a chroma block is half the luma one's side, but never under
//! 4x4: luma split to 4x4 shares one 4x4 chroma block per 8x8 area, coded
//! after the last of its four luma blocks.

use super::super::fidelity;
use super::super::hevc_residual::{self, Kernel};
use super::{read_plane, Block, Corrected, IntraMode, PictureCoder, Plane, Samples};

/// How many times a unit's transform tree may split (both
/// `max_transform_hierarchy_depth`s in the SPS): enough for a 32x32 unit
/// to reach 4x4, the smallest transform. A 16x16 one gets there in two.
pub(super) const MAX_DEPTH: usize = 3;
/// Side of the smallest transform block (the SPS's).
pub(super) const MIN_SIDE: usize = 4;

/// The depths a unit of side `unit` can split to: down to 4x4.
pub(super) fn depths(unit: usize) -> std::ops::RangeInclusive<usize> {
    let to_smallest = (unit / MIN_SIDE).trailing_zeros() as usize;
    0..=to_smallest.min(MAX_DEPTH)
}

/// Roughly what each transform block's flags cost, to weigh a split by.
const FLAG_BITS_PER_BLOCK: usize = 2;

/// The quantised levels of one coding unit: its transform tree split
/// `depth` times evenly, each plane's blocks in z-order (`None` for a block
/// whose levels are all zero).
pub(super) struct Residual {
    pub(super) qp: i32,
    pub(super) depth: usize,
    pub(super) luma: Vec<Option<Vec<i32>>>,
    pub(super) chroma: [Vec<Option<Vec<i32>>>; 2],
}

impl Residual {
    pub(super) fn has_levels(&self) -> bool {
        self.luma
            .iter()
            .chain(self.chroma.iter().flatten())
            .any(Option::is_some)
    }

    /// The bits the levels roughly cost, plus the flags of the blocks a
    /// split adds (an unsplit unit's three are in its header's estimate).
    pub(super) fn estimated_bits(&self) -> usize {
        let blocks = self.luma.iter().chain(self.chroma.iter().flatten());
        let levels: usize = blocks
            .clone()
            .flatten()
            .map(|levels| hevc_residual::estimated_bits(levels))
            .sum();
        levels + FLAG_BITS_PER_BLOCK * (blocks.count() - 3)
    }
}

/// Transform block sides, luma then chroma, of a unit of side `unit` split
/// `depth` times.
pub(super) fn block_sides(unit: usize, depth: usize) -> (usize, usize) {
    let luma = unit >> depth;
    (luma, luma.max(8) / 2)
}

/// The transformed residual of each plane that needs one, block by block
/// in z-order.
pub(super) struct Coefficients {
    pub(super) depth: usize,
    pub(super) luma: Option<Vec<Vec<i64>>>,
    pub(super) chroma: [Option<Vec<Vec<i64>>>; 2],
}

impl Coefficients {
    /// Transform what `prediction` misses of `source` in the planes
    /// `needs` names (luma, chroma), split `depth` times. Always the DCT:
    /// see `PictureCoder::attempt`.
    pub(super) fn of(
        source: &Samples,
        prediction: &Samples,
        [luma, chroma]: [bool; 2],
        depth: usize,
    ) -> Self {
        let unit = source.luma.len().isqrt();
        let (luma_side, chroma_side) = block_sides(unit, depth);
        let transformed = |source: &[u8], prediction: &[u8], side: usize| {
            let residual: Vec<i32> = source
                .iter()
                .zip(prediction)
                .map(|(s, p)| i32::from(*s) - i32::from(*p))
                .collect();
            split_blocks(&residual, side)
                .iter()
                .map(|block| hevc_residual::transform(block, side, Kernel::Dct))
                .collect()
        };
        Self {
            depth,
            luma: luma.then(|| transformed(&source.luma, &prediction.luma, luma_side)),
            chroma: [0, 1].map(|plane| {
                chroma.then(|| {
                    transformed(
                        &source.chroma[plane],
                        &prediction.chroma[plane],
                        chroma_side,
                    )
                })
            }),
        }
    }
}

/// The `side` x `side` blocks of a square plane block, in z-order.
fn split_blocks(samples: &[i32], side: usize) -> Vec<Vec<i32>> {
    let whole = samples.len().isqrt();
    let per_row = whole / side;
    (0..per_row * per_row)
        .map(|index| {
            let (x0, y0) = z_corner(index, side);
            (0..side)
                .flat_map(|y| samples[(y0 + y) * whole + x0..][..side].iter().copied())
                .collect()
        })
        .collect()
}

/// Put `side` x `side` blocks (z-order) back into a square of `whole`.
pub(super) fn join_blocks(blocks: &[Vec<i32>], side: usize, whole: usize) -> Vec<i32> {
    let mut samples = vec![0; whole * whole];
    for (index, block) in blocks.iter().enumerate() {
        let (x0, y0) = z_corner(index, side);
        for y in 0..side {
            samples[(y0 + y) * whole + x0..][..side].copy_from_slice(&block[y * side..][..side]);
        }
    }
    samples
}

/// Where block `index` (z-order) of side `side` starts: z-order counts
/// with x in the even bits and y in the odd ones (6.5.2).
fn z_corner(index: usize, side: usize) -> (usize, usize) {
    (even_bits(index) * side, even_bits(index >> 1) * side)
}

fn even_bits(value: usize) -> usize {
    let mut packed = 0;
    let mut bit = 0;
    while value >> (2 * bit) != 0 {
        packed |= ((value >> (2 * bit)) & 1) << bit;
        bit += 1;
    }
    packed
}

/// The z-order index of the block at column `x`, row `y`. It stops at the
/// highest bit set: intra prediction asks it for every reference sample.
pub(super) fn z_index(x: usize, y: usize) -> usize {
    let mut index = 0;
    let mut bit = 0;
    while (x | y) >> bit != 0 {
        index |= (((x >> bit) & 1) << (2 * bit)) | (((y >> bit) & 1) << (2 * bit + 1));
        bit += 1;
    }
    index
}

/// A block of a split intra unit landed a sample outside the tolerance.
struct OutOfTolerance;

/// One node of the tree being written: its luma corner inside the unit,
/// side, depth, and which of its parent's four it is.
#[derive(Clone, Copy)]
struct Node {
    x: usize,
    y: usize,
    side: usize,
    depth: usize,
    blk_idx: usize,
}

impl PictureCoder<'_> {
    /// An intra unit's residual with its tree split `depth` times, at `qp`,
    /// and what a decoder shows for it: block by block in decoding order,
    /// each predicted in `modes` (luma, chroma) from what is decoded before
    /// it. Leaves the decoder's picture as it found it. `None` when a
    /// sample lands outside the tolerance: given up at the first block
    /// that does, as nothing after it can bring it back.
    pub(super) fn attempt_intra(
        &mut self,
        unit: Block,
        source: &Samples,
        modes: [IntraMode; 2],
        (depth, qp): (usize, i32),
    ) -> Option<Corrected> {
        let before = self.read_samples(&self.shown, unit);
        let residual = self.code_intra_blocks(unit, source, modes, (depth, qp));
        let shown = self.read_samples(&self.shown, unit);
        self.write_shown(unit, &before);
        let residual = residual.ok()?;
        self.is_close(source, &shown).then(|| Corrected {
            bits: residual.estimated_bits(),
            residual,
            shown,
            modes: Some(modes),
        })
    }

    /// `attempt_intra`'s blocks, in decoding order, into the decoder's
    /// picture, until one lands outside the tolerance.
    fn code_intra_blocks(
        &mut self,
        unit: Block,
        source: &Samples,
        [luma_mode, chroma_mode]: [IntraMode; 2],
        (depth, qp): (usize, i32),
    ) -> Result<Residual, OutOfTolerance> {
        let limit = |samples: &[u8]| fidelity::sample_tolerance(samples, self.tolerance);
        let luma_limit = limit(&source.luma);
        let chroma_limits = [limit(&source.chroma[0]), limit(&source.chroma[1])];
        let (luma_side, _) = block_sides(unit.size, depth);
        let chroma_qp = hevc_residual::chroma_qp(qp);
        let mut residual = Residual {
            qp,
            depth,
            luma: Vec::new(),
            chroma: [Vec::new(), Vec::new()],
        };
        for index in 0..(unit.size / luma_side).pow(2) {
            let (x, y) = z_corner(index, luma_side);
            let block = Block {
                x: unit.x + x,
                y: unit.y + y,
                size: luma_side,
            };
            let levels = self.code_intra_block(block, Plane::Luma, luma_mode, (qp, luma_limit))?;
            residual.luma.push(levels);
            // Chroma: this block's own, or with 4x4 luma, after the last
            // of the four, the one their 8x8 area shares.
            let chroma_area = match (luma_side, index % 4) {
                (4, 3) => Block {
                    x: block.x - 4,
                    y: block.y - 4,
                    size: 8,
                },
                (4, _) => continue,
                _ => block,
            };
            for (at, plane) in [Plane::Cb, Plane::Cr].into_iter().enumerate() {
                let bounds = (chroma_qp, chroma_limits[at]);
                let levels = self.code_intra_block(chroma_area, plane, chroma_mode, bounds)?;
                residual.chroma[at].push(levels);
            }
        }
        Ok(residual)
    }

    /// Predict one transform block of `plane` (at `block`, in luma
    /// samples), quantise what the prediction misses at `qp`, and put the
    /// block as a decoder rebuilds it into the decoder's picture: later
    /// blocks predict from it. Its levels, `None` when all are zero; an
    /// error when a sample lands more than `limit` off.
    fn code_intra_block(
        &mut self,
        block: Block,
        plane: Plane,
        mode: IntraMode,
        (qp, limit): (i32, i32),
    ) -> Result<Option<Vec<i32>>, OutOfTolerance> {
        let is_luma = plane == Plane::Luma;
        let (_, _, side) = plane.scaled(block);
        let prediction = self.references(block, plane).predict(mode, side, is_luma);
        let source = read_plane(self.source, block, plane);
        let missed: Vec<i32> = source
            .iter()
            .zip(&prediction)
            .map(|(s, p)| i32::from(*s) - i32::from(*p))
            .collect();
        let kernel = Kernel::of(side, is_luma);
        let coefficients = hevc_residual::transform(&missed, side, kernel);
        let levels = hevc_residual::quantize(&coefficients, side, qp);
        let has_levels = levels.iter().any(|level| *level != 0);
        let shown: Vec<u8> = if has_levels {
            let rebuilt = hevc_residual::rebuild(&levels, side, qp, kernel);
            prediction
                .iter()
                .zip(&rebuilt)
                .map(|(p, r)| (i32::from(*p) + r).clamp(0, 255) as u8)
                .collect()
        } else {
            prediction
        };
        let is_too_far = source
            .iter()
            .zip(&shown)
            .any(|(s, d)| i32::from(s.abs_diff(*d)) > limit);
        if is_too_far {
            return Err(OutOfTolerance);
        }
        self.write_shown_plane(block, plane, &shown);
        Ok(has_levels.then_some(levels))
    }

    /// transform_tree() of the unit at `block`. With no residual every
    /// coded block flag is 0, which only intra units may send.
    pub(super) fn code_transform_tree(
        &mut self,
        block: Block,
        residual: Option<&Residual>,
        is_inter: bool,
    ) {
        let none = Residual {
            qp: self.qp,
            depth: 0,
            luma: vec![None],
            chroma: [vec![None], vec![None]],
        };
        let root = Node {
            x: 0,
            y: 0,
            side: block.size,
            depth: 0,
            blk_idx: 0,
        };
        let tree = Tree {
            unit: block.size,
            residual: residual.unwrap_or(&none),
            is_inter,
        };
        self.code_node(&tree, root, [true, true]);
    }

    /// One node: its split flag, its chroma coded block flags (each sent
    /// only when the parent's was 1), then its four children or its
    /// transform unit.
    fn code_node(&mut self, tree: &Tree, node: Node, parent_chroma: [bool; 2]) {
        let log2 = node.side.trailing_zeros() as usize;
        let is_split = node.depth < tree.residual.depth;
        if log2 > 2 && node.depth < MAX_DEPTH {
            let ctx = &mut self.contexts.split_transform[5 - log2];
            self.cabac.decision(ctx, is_split);
        } else {
            debug_assert!(!is_split, "a split the syntax can't signal");
        }
        let chroma = if log2 > 2 {
            [0, 1].map(|plane| {
                if node.depth > 0 && !parent_chroma[plane] {
                    return false;
                }
                let has_levels = tree.chroma_has_levels(plane, node);
                let ctx = &mut self.contexts.cbf_chroma[node.depth];
                self.cabac.decision(ctx, has_levels);
                has_levels
            })
        } else {
            parent_chroma
        };
        if is_split {
            let half = node.side / 2;
            for blk_idx in 0..4 {
                let child = Node {
                    x: node.x + (blk_idx & 1) * half,
                    y: node.y + (blk_idx >> 1) * half,
                    side: half,
                    depth: node.depth + 1,
                    blk_idx,
                };
                self.code_node(tree, child, chroma);
            }
            return;
        }
        self.code_leaf(tree, node, chroma);
    }

    /// transform_unit(): cbf_luma (inferred 1 for an inter unit's single
    /// block when neither chroma flag is set), the QP delta before the
    /// quantisation group's first levels, then the levels.
    fn code_leaf(&mut self, tree: &Tree, node: Node, chroma: [bool; 2]) {
        let luma = &tree.residual.luma[z_index(node.x / node.side, node.y / node.side)];
        if !tree.is_inter || node.depth > 0 || chroma[0] || chroma[1] {
            let ctx = &mut self.contexts.cbf_luma[usize::from(node.depth == 0)];
            self.cabac.decision(ctx, luma.is_some());
        } else {
            debug_assert!(luma.is_some(), "an inferred cbf_luma needs levels");
        }
        if (luma.is_some() || chroma[0] || chroma[1]) && !self.is_qp_coded {
            self.code_qp_delta(tree.residual.qp);
        }
        let contexts = &mut self.contexts.residual;
        if let Some(levels) = luma {
            hevc_residual::write(&mut self.cabac, contexts, levels, node.side, false);
        }
        // Chroma: this node's own block, or with 4x4 luma, after the last
        // of the four, the one their 8x8 area shares.
        let chroma_area = match (node.side, node.blk_idx) {
            (4, 3) => Some((node.x - 4, node.y - 4, 8)),
            (4, _) => None,
            (side, _) => Some((node.x, node.y, side)),
        };
        let Some((x, y, area)) = chroma_area else {
            return;
        };
        for plane in 0..2 {
            let index = z_index(x / area, y / area);
            if let Some(levels) = &tree.residual.chroma[plane][index] {
                hevc_residual::write(&mut self.cabac, contexts, levels, area / 2, true);
            }
        }
    }
}

/// What every node of one unit's tree reads.
struct Tree<'a> {
    unit: usize,
    residual: &'a Residual,
    is_inter: bool,
}

impl Tree<'_> {
    /// Whether any chroma block of `plane` inside `node` has levels.
    fn chroma_has_levels(&self, plane: usize, node: Node) -> bool {
        let (_, chroma_side) = block_sides(self.unit, self.residual.depth);
        // The luma area each chroma block covers.
        let area = 2 * chroma_side;
        let per_node = (node.side / area).pow(2);
        let first = z_index(node.x / node.side, node.y / node.side) * per_node;
        self.residual.chroma[plane][first..first + per_node]
            .iter()
            .any(Option::is_some)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn z_order_runs_left_right_then_down_at_every_level() {
        let corners: Vec<(usize, usize)> = (0..8).map(|i| z_corner(i, 1)).collect();
        assert_eq!(
            corners,
            [
                (0, 0),
                (1, 0),
                (0, 1),
                (1, 1),
                (2, 0),
                (3, 0),
                (2, 1),
                (3, 1)
            ]
        );
        for index in 0..64 {
            let (x, y) = z_corner(index, 1);
            assert_eq!(z_index(x, y), index);
        }
    }

    #[test]
    fn splitting_and_joining_gives_the_block_back() {
        let samples: Vec<i32> = (0..256).collect();
        for side in [4, 8, 16] {
            let blocks = split_blocks(&samples, side);
            assert_eq!(blocks.len(), (16 / side).pow(2));
            assert_eq!(join_blocks(&blocks, side, 16), samples);
        }
    }

    #[test]
    fn chroma_never_goes_under_four() {
        assert_eq!(block_sides(16, 0), (16, 8));
        assert_eq!(block_sides(16, 1), (8, 4));
        assert_eq!(block_sides(16, 2), (4, 4));
        assert_eq!(block_sides(32, 2), (8, 4));
        assert_eq!(block_sides(32, 3), (4, 4));
    }

    #[test]
    fn units_split_down_to_four_and_no_further() {
        assert_eq!(depths(32), 0..=3);
        assert_eq!(depths(16), 0..=2);
        assert_eq!(depths(8), 0..=1);
    }
}
