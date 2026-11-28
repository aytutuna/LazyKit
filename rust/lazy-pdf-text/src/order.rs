//! Reading order of blocks by recursive cuts, across the widest empty bands first, then between columns. Bands that are
//! columnar on both sides of a cut and share a gutter are kept together, otherwise two columns whose paragraph gaps
//! happen to line up would be read across.

use std::collections::BTreeMap;

use crate::flow::{FlowBlock, FlowSeg};
use crate::unicode::compare_doubles;

const MAX_DEPTH: usize = 48;
const MAX_BLOCKS: usize = 20_000;

struct Order<'a> {
    blocks: &'a [FlowBlock],
    em: f64,
    rtl: bool,
    result: Vec<usize>,
}

struct Band {
    items: Vec<usize>,
    merged: bool,
}

type Gutter = (f64, f64);

/// The blocks as indexes in reading order. In a right-to-left page the cells of a table run from the right.
pub fn sort(blocks: &mut [FlowBlock], segs: &[FlowSeg], em: f64, rtl: bool) -> Vec<usize> {
    if rtl {
        for b in blocks.iter_mut().filter(|b| b.table) {
            b.segs.sort_by(|&x, &y| {
                let (x, y) = (&segs[x], &segs[y]);
                if x.row != y.row {
                    x.row.cmp(&y.row)
                } else {
                    compare_doubles(y.x0, x.x0)
                }
            });
        }
    }
    let mut order = Order {
        blocks,
        em,
        rtl,
        result: Vec::new(),
    };
    let all: Vec<usize> = (0..blocks.len()).collect();
    if blocks.len() > MAX_BLOCKS {
        order.leaf(&all);
    } else {
        order.cut(&all, true, 0);
    }
    order.result
}

impl Order<'_> {
    fn cut(&mut self, items: &[usize], horizontal: bool, depth: usize) {
        if items.len() <= 1 {
            self.result.extend_from_slice(items);
            return;
        }
        if depth > MAX_DEPTH {
            self.leaf(items);
            return;
        }
        if horizontal {
            let bands = self.merge(self.bands(items));
            if bands.len() > 1 || bands.first().is_some_and(|b| b.merged) {
                for band in bands {
                    self.cut(&band.items, !band.merged, depth + 1);
                }
                return;
            }
        }
        let mut columns = self.columns(items);
        if columns.len() < 2 {
            self.leaf(items);
            return;
        }
        if self.rtl {
            columns.reverse();
        }
        for column in columns {
            self.cut(&column, true, depth + 1);
        }
    }

    fn bands(&self, items: &[usize]) -> Vec<Band> {
        let mut sorted = items.to_vec();
        self.sort_top_left(&mut sorted);
        let mut bands = Vec::new();
        let mut current: Vec<usize> = Vec::new();
        let mut bottom = -f64::MAX;
        for b in sorted {
            let block = &self.blocks[b];
            if !current.is_empty() && block.ya > bottom + 0.05 * self.em {
                bands.push(Band {
                    items: std::mem::take(&mut current),
                    merged: false,
                });
            }
            current.push(b);
            bottom = if current.len() == 1 {
                block.yb
            } else {
                bottom.max(block.yb)
            };
        }
        bands.push(Band {
            items: current,
            merged: false,
        });
        bands
    }

    fn sort_top_left(&self, items: &mut [usize]) {
        items.sort_by(|&a, &b| {
            let (a, b) = (&self.blocks[a], &self.blocks[b]);
            compare_doubles(a.ya, b.ya).then(compare_doubles(a.x0, b.x0))
        });
    }

    fn merge(&self, bands: Vec<Band>) -> Vec<Band> {
        if bands.len() < 2 {
            return bands;
        }
        let mut merged: Vec<Band> = Vec::new();
        let mut previous: Option<Vec<Gutter>> = None;
        for band in bands {
            let gutters = self.gutters(&self.columns(&band.items));
            if let Some(last) = merged.last_mut()
                && let Some(before) = &previous
                && !before.is_empty()
                && !gutters.is_empty()
                && self.shares(before, &gutters)
            {
                last.items.extend(band.items);
                last.merged = true;
            } else {
                merged.push(band);
            }
            previous = Some(gutters);
        }
        merged
    }

    fn shares(&self, a: &[Gutter], b: &[Gutter]) -> bool {
        a.iter().any(|x| {
            b.iter()
                .any(|y| x.1.min(y.1) - x.0.max(y.0) > 0.3 * self.em)
        })
    }

    fn gutters(&self, columns: &[Vec<usize>]) -> Vec<Gutter> {
        let mut result = Vec::new();
        for i in 1..columns.len() {
            let left = columns[i - 1]
                .iter()
                .map(|&b| self.blocks[b].x1)
                .fold(None, |m: Option<f64>, v| {
                    Some(m.map_or(v, |m| if v > m { v } else { m }))
                });
            let right = columns[i]
                .iter()
                .map(|&b| self.blocks[b].x0)
                .fold(None, |m: Option<f64>, v| {
                    Some(m.map_or(v, |m| if v < m { v } else { m }))
                });
            result.push((left.unwrap_or(0.0), right.unwrap_or(0.0)));
        }
        result
    }

    // Gutters are stretches of x that almost nothing covers, so that a page number sitting in one does not make two
    // columns one. A block belongs to the column its centre is in.
    fn columns(&self, items: &[usize]) -> Vec<Vec<usize>> {
        let limit = 0.04
            * items
                .iter()
                .fold(0.0, |s, &b| s + (self.blocks[b].yb - self.blocks[b].ya));
        let mut edges: Vec<(f64, f64)> = Vec::with_capacity(items.len() * 2);
        for &b in items {
            let b = &self.blocks[b];
            edges.push((b.x0, b.yb - b.ya));
            edges.push((b.x1, b.ya - b.yb));
        }
        edges.sort_by(|a, b| compare_doubles(a.0, b.0));
        let mut xs: Vec<f64> = Vec::new();
        let mut cover: Vec<f64> = Vec::new();
        let mut sum = 0.0;
        let mut i = 0;
        while i < edges.len() {
            let x = edges[i].0;
            loop {
                sum += edges[i].1;
                i += 1;
                if !(i < edges.len() && edges[i].0 == x) {
                    break;
                }
            }
            xs.push(x);
            cover.push(sum);
        }
        let mut gutters: Vec<f64> = Vec::new();
        let mut i = 0;
        while i + 1 < xs.len() {
            if cover[i] > limit {
                i += 1;
                continue;
            }
            let mut j = i + 1;
            while j + 1 < xs.len() && cover[j] <= limit {
                j += 1;
            }
            if i > 0 && j + 1 < xs.len() && xs[j] - xs[i] > 0.9 * self.em {
                gutters.push((xs[i] + xs[j]) / 2.0);
            }
            i = j;
        }
        let mut by_column: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
        for &b in items {
            let block = &self.blocks[b];
            let centre = (block.x0 + block.x1) / 2.0;
            let k = gutters.iter().filter(|&&g| g < centre).count();
            by_column.entry(k).or_default().push(b);
        }
        by_column.into_values().collect()
    }

    // Nothing separates these: rows from the top, each row left to right.
    fn leaf(&mut self, items: &[usize]) {
        let mut sorted = items.to_vec();
        self.sort_top_left(&mut sorted);
        let mut row: Vec<usize> = Vec::new();
        let mut head: Option<usize> = None;
        for b in sorted {
            if let Some(h) = head {
                let (h, block) = (&self.blocks[h], &self.blocks[b]);
                if block.ya >= h.ya + 0.5 * (h.yb - h.ya) {
                    self.flush(&mut row);
                }
            }
            if row.is_empty() {
                head = Some(b);
            }
            row.push(b);
        }
        self.flush(&mut row);
    }

    fn flush(&mut self, row: &mut Vec<usize>) {
        let blocks = self.blocks;
        if self.rtl {
            row.sort_by(|&a, &b| compare_doubles(blocks[b].x1, blocks[a].x1));
        } else {
            row.sort_by(|&a, &b| compare_doubles(blocks[a].x0, blocks[b].x0));
        }
        self.result.append(row);
    }
}
