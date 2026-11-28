//! Groups segments into tables and paragraph chains.

use crate::flow::{FlowBlock, FlowSeg, median};
use crate::unicode::compare_doubles;

const MAX_ROW_CHARS: f64 = 24.0;
const MAX_ROW_WORDS: f64 = 3.0;
const MAX_LOOK_BACK: usize = 200;

pub fn blocks(lines: &[Vec<usize>], em: f64, segs: &[FlowSeg]) -> Vec<FlowBlock> {
    let mut blocks = Vec::new();
    let mut loose: Vec<usize> = Vec::new();
    let mut run: Vec<&Vec<usize>> = Vec::new();

    fn flush(
        run: &mut Vec<&Vec<usize>>,
        blocks: &mut Vec<FlowBlock>,
        loose: &mut Vec<usize>,
        segs: &[FlowSeg],
    ) {
        let table = run.len() >= 3 || (run.len() == 2 && run.iter().all(|l| l.len() >= 3));
        if table {
            let mut block = FlowBlock::new();
            block.table = true;
            for line in run.iter() {
                for &s in line.iter() {
                    block.add(s, &segs[s]);
                }
            }
            blocks.push(block);
        } else {
            for line in run.iter() {
                loose.extend(line.iter().copied());
            }
        }
        run.clear();
    }

    for (i, line) in lines.iter().enumerate() {
        let row = is_row(line, segs);
        if row && let Some(tail) = run.last() {
            let top = line
                .iter()
                .map(|&s| segs[s].ya)
                .fold(None, min_of)
                .unwrap_or(0.0);
            let bottom = tail
                .iter()
                .map(|&s| segs[s].yb)
                .fold(None, max_of)
                .unwrap_or(0.0);
            if top - bottom > 1.5 * em {
                flush(&mut run, &mut blocks, &mut loose, segs);
            }
        }
        if row {
            run.push(line);
            continue;
        }
        // A row with one filled cell stays in the table when more rows follow in line with it.
        if let Some(tail) = run.last()
            && line.len() == 1
            && i + 1 < lines.len()
            && is_row(&lines[i + 1], segs)
            && aligns_with(&segs[line[0]], tail, segs)
        {
            run.push(line);
            continue;
        }
        flush(&mut run, &mut blocks, &mut loose, segs);
        loose.extend(line.iter().copied());
    }
    flush(&mut run, &mut blocks, &mut loose, segs);
    blocks.extend(chains(loose, segs));
    blocks
}

fn min_of(m: Option<f64>, v: f64) -> Option<f64> {
    Some(m.map_or(v, |m| if v < m { v } else { m }))
}

fn max_of(m: Option<f64>, v: f64) -> Option<f64> {
    Some(m.map_or(v, |m| if v > m { v } else { m }))
}

fn is_row(line: &[usize], segs: &[FlowSeg]) -> bool {
    line.len() >= 2
        && median(line.iter().map(|&s| segs[s].text as f64).collect()) <= MAX_ROW_CHARS
        && median(line.iter().map(|&s| segs[s].words as f64).collect()) <= MAX_ROW_WORDS
}

fn aligns_with(cell: &FlowSeg, row: &[usize], segs: &[FlowSeg]) -> bool {
    row.iter()
        .any(|&s| (segs[s].x0 - cell.x0).abs() < 0.6 * cell.em)
}

// Each segment hangs under the one above it that it overlaps most, at most one child per parent so that text under a
// heading that spans two columns does not join both columns.
fn chains(mut order: Vec<usize>, all: &[FlowSeg]) -> Vec<FlowBlock> {
    order.sort_by(|&a, &b| {
        let (a, b) = (&all[a], &all[b]);
        if a.y != b.y {
            compare_doubles(a.y, b.y)
        } else {
            compare_doubles(a.x0, b.x0)
        }
    });
    let n = order.len();
    let mut candidates: Vec<Vec<(usize, f64, f64)>> = vec![Vec::new(); n];
    let mut nearest: Vec<f64> = Vec::new();
    for i in 0..n {
        let s = &all[order[i]];
        let mut closest = f64::MAX;
        let low = i.saturating_sub(MAX_LOOK_BACK);
        for j in (low..i).rev() {
            let p = &all[order[j]];
            let dy = s.y - p.y;
            if dy > 2.6 * s.em.max(p.em) {
                break;
            }
            let ratio = s.em / p.em;
            let overlap = s.x1.min(p.x1) - s.x0.max(p.x0);
            if dy < 0.5 * s.em.max(p.em) || ratio < 0.7 || ratio > 1.43 {
                continue;
            }
            if overlap <= 0.0 || overlap < 0.3 * s.width().min(p.width()) {
                continue;
            }
            candidates[i].push((j, overlap, dy));
            closest = closest.min(dy);
        }
        if closest < f64::MAX {
            nearest.push(closest);
        }
    }
    // A gap well past the usual line pitch is a paragraph break.
    nearest.sort_by(|a, b| compare_doubles(*a, *b));
    let pitch = if nearest.is_empty() {
        0.0
    } else {
        nearest[nearest.len() / 2]
    };
    let mut parent: Vec<Option<usize>> = vec![None; n];
    let mut has_child = vec![false; n];
    for i in 0..n {
        let mut best: Option<usize> = None;
        let mut best_overlap = 0.0;
        for &(j, overlap, dy) in &candidates[i] {
            if has_child[j] || dy > (1.45 * pitch).max(1.3 * all[order[i]].em) {
                continue;
            }
            if overlap > best_overlap {
                best_overlap = overlap;
                best = Some(j);
            }
        }
        let Some(best) = best else { continue };
        has_child[best] = true;
        parent[i] = Some(best);
    }

    let mut owner: Vec<usize> = vec![0; n];
    let mut result: Vec<FlowBlock> = Vec::new();
    for i in 0..n {
        let block = match parent[i] {
            Some(p) => owner[p],
            None => {
                result.push(FlowBlock::new());
                result.len() - 1
            }
        };
        result[block].add(order[i], &all[order[i]]);
        owner[i] = block;
    }
    result
}
