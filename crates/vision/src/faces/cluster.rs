//! Grouping faces into people: the sharpest, most certain faces start a person, and each other
//! face joins the person it looks most like, if it looks enough like them. Deterministic (the same
//! faces always make the same people), and cheap for a family library (a few thousand faces);
//! the cost is faces × people × 128 multiplications.
// ponytail: leader clustering with one merge pass; a hierarchical or graph method (Chinese
// Whispers) if clusters of one person come out split on real libraries, and an index over the
// centroids beyond ~100k faces.

use super::DIM;
use super::index::Face;
use crate::store::Key;

/// A face joins a person when its similarity (cosine) with their average face is at least this.
/// SFace's own threshold for "the same person" between two photos is 0.363; a person's average
/// is a steadier reference than one photo, so this is a little stricter, which keeps strangers
/// apart at the cost of splitting a person now and then (naming both with one name joins them).
pub const JOIN: f32 = 0.40;
/// Two people whose averages are this close are one (a second look after all faces are placed).
pub const MERGE: f32 = 0.48;
/// The merge look is skipped with more people than this (it compares every pair).
const MERGE_MAX: usize = 3000;

/// A group of faces that look like one person.
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    /// Indices into the faces that were grouped, the best face first.
    pub members: Vec<usize>,
    /// The average face, unit length.
    pub centroid: Vec<f32>,
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn unit(sum: &[f32]) -> Vec<f32> {
    let n = dot(sum, sum).sqrt();
    if n > 1e-9 { sum.iter().map(|x| x / n).collect() } else { sum.to_vec() }
}

/// How good a face is to start a person with: sure detections, big faces.
fn quality(f: &Face) -> f32 {
    f.score * ((f.rect[2] - f.rect[0]) * (f.rect[3] - f.rect[1])).max(0.0).sqrt()
}

/// `faces` as people, biggest first (ties: the better face first).
pub fn group(faces: &[Face], join: f32) -> Vec<Group> {
    let items: Vec<(&[f32], f32)> = faces.iter().map(|f| (if f.embedding.len() == DIM { &f.embedding[..] } else { &[][..] }, quality(f))).collect();
    group_vectors(&items, join)
}

/// Unit vectors as people, each `(vector, quality)`: the best faces start a person, a face joins the person whose
/// average it is at least `join` like, and two people whose averages are at least `join + (MERGE - JOIN)` alike are one.
/// Members are indices into `items`, the best first; the biggest group first. A vector that is empty or not as long as
/// the first one is left out (it belongs to no group).
pub fn group_vectors(items: &[(&[f32], f32)], join: f32) -> Vec<Group> {
    let dim = items.iter().map(|(v, _)| v.len()).find(|&n| n > 0).unwrap_or(0);
    let q = |i: usize| items.get(i).map_or(0.0, |it| it.1);
    let mut order: Vec<usize> = (0..items.len()).filter(|&i| items.get(i).is_some_and(|(v, _)| dim > 0 && v.len() == dim)).collect();
    order.sort_by(|&a, &b| q(b).total_cmp(&q(a)).then(a.cmp(&b)));
    let mut sums: Vec<Vec<f32>> = Vec::new();
    let mut groups: Vec<Group> = Vec::new();
    for i in order {
        let Some((v, _)) = items.get(i) else { continue };
        let best = groups.iter().enumerate().map(|(g, grp)| (g, dot(v, &grp.centroid))).max_by(|a, b| a.1.total_cmp(&b.1));
        match best {
            Some((g, sim)) if sim >= join => {
                if let (Some(sum), Some(grp)) = (sums.get_mut(g), groups.get_mut(g)) {
                    sum.iter_mut().zip(v.iter()).for_each(|(s, e)| *s += e);
                    grp.centroid = unit(sum);
                    grp.members.push(i);
                }
            }
            _ => {
                sums.push(v.to_vec());
                groups.push(Group { members: vec![i], centroid: v.to_vec() });
            }
        }
    }
    if groups.len() <= MERGE_MAX {
        merge_close(&mut groups, &mut sums, join + (MERGE - JOIN));
    }
    for g in &mut groups {
        g.members.sort_by(|&a, &b| q(b).total_cmp(&q(a)).then(a.cmp(&b)));
    }
    groups.sort_by(|a, b| b.members.len().cmp(&a.members.len()).then_with(|| a.members.first().cmp(&b.members.first())));
    groups
}

/// Joins people whose averages are `merge` close: one look at every pair.
fn merge_close(groups: &mut Vec<Group>, sums: &mut Vec<Vec<f32>>, merge: f32) {
    let mut a = 0;
    while a < groups.len() {
        let mut b = a + 1;
        while b < groups.len() {
            let close = matches!((groups.get(a), groups.get(b)), (Some(x), Some(y)) if dot(&x.centroid, &y.centroid) >= merge);
            if !close {
                b += 1;
                continue;
            }
            let (gone, sum_gone) = (groups.remove(b), sums.remove(b));
            if let (Some(keep), Some(sum)) = (groups.get_mut(a), sums.get_mut(a)) {
                sum.iter_mut().zip(&sum_gone).for_each(|(s, e)| *s += e);
                keep.centroid = unit(sum);
                keep.members.extend(gone.members);
            }
        }
        a += 1;
    }
}

/// The name of a face to refer to it by (`<photo key>.<face number>`): a person is named by the id
/// of their best face.
pub fn face_id(f: &Face) -> String {
    format!("{}.{}", f.key.to_hex(), f.index)
}

/// The photo key and face number in a [`face_id`].
pub fn parse_face_id(id: &str) -> Option<(Key, u8)> {
    let (key, index) = id.split_once('.')?;
    Some((Key::from_hex(key)?, index.parse().ok()?))
}
