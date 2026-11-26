// Lazy Scan's MaskOpsTests: the operations against plain per-pixel definitions on random masks, and the worked cases.
use lazy_scan::mask_ops::{component_areas, connected_components, dilate, erode};

fn random_mask(w: usize, h: usize, percent: u32, seed: u32) -> Vec<u8> {
    let mut s = seed;
    (0..w * h)
        .map(|_| {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            ((s >> 16) % 100 < percent) as u8
        })
        .collect()
}

const MASKS: [(usize, usize, u32, u32); 10] = [
    (1, 1, 50, 1),
    (7, 1, 50, 2),
    (1, 9, 50, 3),
    (13, 11, 30, 4),
    (64, 48, 10, 5),
    (64, 48, 50, 6),
    (64, 48, 85, 7),
    (101, 37, 60, 8),
    (5, 5, 100, 9),
    (5, 5, 0, 10),
];

// the square around (x, y), `None` for a pixel outside the image
fn window(
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    r: usize,
) -> impl Iterator<Item = Option<(usize, usize)>> {
    let (x, y, r) = (x as i64, y as i64, r as i64);
    (y - r..=y + r).flat_map(move |yy| {
        (x - r..=x + r).map(move |xx| {
            let inside = xx >= 0 && yy >= 0 && (xx as usize) < w && (yy as usize) < h;
            inside.then_some((xx as usize, yy as usize))
        })
    })
}

fn slow_dilate(mask: &[u8], w: usize, h: usize, r: usize) -> Vec<u8> {
    let mut out = vec![0; w * h];
    for y in 0..h {
        for x in 0..w {
            let any = window(w, h, x, y, r)
                .flatten()
                .any(|(a, b)| mask[b * w + a] == 1);
            out[y * w + x] = any as u8;
        }
    }
    out
}

fn slow_erode(mask: &[u8], w: usize, h: usize, r: usize, outside_set: bool) -> Vec<u8> {
    let mut out = vec![0; w * h];
    for y in 0..h {
        for x in 0..w {
            let all = window(w, h, x, y, r).all(|p| match p {
                Some((a, b)) => mask[b * w + a] == 1,
                None => outside_set,
            });
            out[y * w + x] = all as u8;
        }
    }
    out
}

#[test]
fn dilate_and_erode_match_their_definitions() {
    for (w, h, percent, seed) in MASKS {
        let mask = random_mask(w, h, percent, seed);
        for r in [0, 1, 2, 3, 7] {
            assert_eq!(dilate(&mask, w, h, r), slow_dilate(&mask, w, h, r));
            assert_eq!(
                erode(&mask, w, h, r, false),
                slow_erode(&mask, w, h, r, false)
            );
            assert_eq!(
                erode(&mask, w, h, r, true),
                slow_erode(&mask, w, h, r, true)
            );
        }
    }
}

#[test]
fn components_are_found_in_order_with_their_extents() {
    for (w, h, percent, seed) in MASKS {
        let mask = random_mask(w, h, percent, seed);
        let comps = connected_components(&mask, w, h);
        let areas = component_areas(&mask, w, h, false);
        assert_eq!(comps.iter().map(|c| c.area).collect::<Vec<_>>(), areas);
        let set = mask.iter().filter(|&&v| v == 1).count();
        assert_eq!(areas.iter().sum::<usize>(), set);
        let eight = component_areas(&mask, w, h, true);
        assert_eq!(eight.iter().sum::<usize>(), set);
        assert!(eight.len() <= areas.len());
        // the first pixel of each component, in row-major order, starts it
        let mut firsts = comps.iter().map(|c| {
            let (&y, &(x, _)) = c.rows.iter().next().unwrap();
            y * w + x
        });
        let mut previous = None;
        for first in firsts.by_ref() {
            assert!(previous.is_none_or(|p| p < first));
            previous = Some(first);
        }
    }
}

#[test]
fn components_are_four_connected_or_eight() {
    let diagonal = [1, 0, 0, 1];
    assert_eq!(component_areas(&diagonal, 2, 2, false), [1, 1]);
    assert_eq!(component_areas(&diagonal, 2, 2, true), [2]);
    let anti = [0, 1, 1, 0];
    assert_eq!(component_areas(&anti, 2, 2, true), [2]);
    let mut staircase = vec![0u8; 36];
    for i in 0..6 {
        staircase[i * 6 + i] = 1;
    }
    assert_eq!(component_areas(&staircase, 6, 6, true), [6]);
    assert_eq!(component_areas(&staircase, 6, 6, false).len(), 6);
    let plus = [0, 1, 0, 1, 1, 1, 0, 1, 0];
    assert_eq!(component_areas(&plus, 3, 3, false), [5]);
    let rows: Vec<_> = connected_components(&plus, 3, 3)[0]
        .rows
        .iter()
        .map(|(y, (a, b))| [*y, *a, *b])
        .collect();
    assert_eq!(rows, [[0, 1, 1], [1, 0, 2], [2, 1, 1]]);
}

#[test]
fn erode_with_the_outside_set_keeps_the_edge_of_a_region_against_the_border() {
    let mut mask = vec![0u8; 80];
    for y in 0..4 {
        for x in 0..6 {
            mask[y * 10 + x] = 1;
        }
    }
    let mut kept = vec![0u8; 80];
    let mut shrunk = vec![0u8; 80];
    for y in 0..8 {
        for x in 0..10 {
            kept[y * 10 + x] = (x < 5 && y < 3) as u8;
            shrunk[y * 10 + x] = ((1..5).contains(&x) && (1..3).contains(&y)) as u8;
        }
    }
    assert_eq!(erode(&mask, 10, 8, 1, true), kept);
    assert_eq!(erode(&mask, 10, 8, 1, false), shrunk);
}

#[test]
fn erode_shrinks_at_the_borders_and_dilate_does_not_grow_past_them() {
    let full = vec![1u8; 25];
    let eroded = erode(&full, 5, 5, 1, false);
    assert_eq!(eroded.iter().filter(|&&v| v == 1).count(), 9);
    assert_eq!((eroded[0], eroded[12]), (0, 1));
    let mut dot = vec![0u8; 25];
    dot[12] = 1;
    assert_eq!(dilate(&dot, 5, 5, 1).iter().filter(|&&v| v == 1).count(), 9);
    assert_eq!(
        dilate(&dot, 5, 5, 2).iter().filter(|&&v| v == 1).count(),
        25
    );
}
