// ── src/ann.rs : approximate nearest neighbour index (Pillar 3, scale) ──────
//
// Brute-force cosine over every stored vector is fine at thousands of rows
// (sub-millisecond), but it is O(n) per query and dies at hundreds of thousands.
// This wraps a pure-Rust HNSW index (instant-distance, no C deps -> stays
// zero-install) so semantic search stays fast as the corpus grows. It activates
// only above a size threshold; small corpora keep using exact brute force.
//
// AnnCache holds a built index + its row metadata + the row count it was built
// for, so the memory actor rebuilds it only when the document set changes.

use instant_distance::{Builder, HnswMap, Point, Search};

#[derive(Clone, Debug)]
pub struct EmbPoint(pub Vec<f32>);

impl Point for EmbPoint {
    // instant-distance wants a distance (smaller = closer). We use cosine
    // DISTANCE = 1 - cosine similarity, so it ranks identically to cosine.
    fn distance(&self, other: &Self) -> f32 {
        let (a, b) = (&self.0, &other.0);
        let n = a.len().min(b.len());
        let mut dot = 0.0f32;
        let mut na = 0.0f32;
        let mut nb = 0.0f32;
        for i in 0..n {
            dot += a[i] * b[i];
            na += a[i] * a[i];
            nb += b[i] * b[i];
        }
        let denom = (na.sqrt() * nb.sqrt()).max(1e-8);
        1.0 - dot / denom
    }
}

pub struct AnnIndex {
    map: HnswMap<EmbPoint, usize>,
}

impl AnnIndex {
    // Build an index over `vectors`; the value stored at each point is its index
    // into the original slice, so callers can map results back to their metadata.
    pub fn build(vectors: Vec<Vec<f32>>) -> Self {
        let points: Vec<EmbPoint> = vectors.into_iter().map(EmbPoint).collect();
        let values: Vec<usize> = (0..points.len()).collect();
        let map = Builder::default().build(points, values);
        AnnIndex { map }
    }

    // Top-k nearest by cosine. Returns (original_index, cosine_similarity).
    pub fn search(&self, query: &[f32], k: usize) -> Vec<(usize, f32)> {
        let mut search = Search::default();
        let q = EmbPoint(query.to_vec());
        self.map
            .search(&q, &mut search)
            .take(k.max(1))
            .map(|item| (*item.value, 1.0 - item.distance))
            .collect()
    }
}

// Cached index for the memory actor: rebuilt only when `built_for` (the row count
// it was built against) no longer matches the table.
#[derive(Default)]
pub struct AnnCache {
    pub built_for: usize,
    pub index: Option<AnnIndex>,
    pub meta: Vec<(String, String)>, // (source, chunk) aligned with point indices
}

impl AnnCache {
    pub fn rebuild(&mut self, count: usize, vectors: Vec<Vec<f32>>, meta: Vec<(String, String)>) {
        self.index = Some(AnnIndex::build(vectors));
        self.meta = meta;
        self.built_for = count;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cos(a: &[f32], b: &[f32]) -> f32 {
        let mut d = 0.0;
        let mut na = 0.0;
        let mut nb = 0.0;
        for i in 0..a.len() {
            d += a[i] * b[i];
            na += a[i] * a[i];
            nb += b[i] * b[i];
        }
        d / (na.sqrt() * nb.sqrt()).max(1e-8)
    }

    #[test]
    fn ann_top1_matches_brute_force() {
        // 60 distinct points in 8-D; query equals one of them, so the true
        // nearest is that exact point - HNSW must find it.
        let mut vecs = Vec::new();
        for i in 0..60usize {
            let mut v = vec![0.0f32; 8];
            v[i % 8] = 1.0 + (i as f32) * 0.01;
            v[(i * 3) % 8] += 0.2 + (i as f32) * 0.005;
            vecs.push(v);
        }
        let idx = AnnIndex::build(vecs.clone());
        let q = vecs[17].clone();
        let ann = idx.search(&q, 1);
        let mut best = (0usize, f32::MIN);
        for (i, v) in vecs.iter().enumerate() {
            let c = cos(&q, v);
            if c > best.1 {
                best = (i, c);
            }
        }
        assert_eq!(ann[0].0, best.0);
        assert!((ann[0].1 - 1.0).abs() < 1e-3); // cosine with itself ~ 1.0
    }

    #[test]
    fn ann_high_recall_topk() {
        // Random-ish set; ANN top-5 should overlap brute-force top-5 strongly.
        let mut vecs = Vec::new();
        for i in 0..200usize {
            let mut v = vec![0.0f32; 16];
            for (j, x) in v.iter_mut().enumerate() {
                *x = (((i * 7 + j * 13) % 97) as f32) / 97.0;
            }
            vecs.push(v);
        }
        let idx = AnnIndex::build(vecs.clone());
        let q = vecs[42].clone();
        let ann: Vec<usize> = idx.search(&q, 5).into_iter().map(|(i, _)| i).collect();
        let mut bf: Vec<(usize, f32)> = vecs.iter().enumerate().map(|(i, v)| (i, cos(&q, v))).collect();
        bf.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        let bf_top: Vec<usize> = bf.into_iter().take(5).map(|(i, _)| i).collect();
        let overlap = ann.iter().filter(|i| bf_top.contains(i)).count();
        assert!(overlap >= 4, "ANN/brute-force top-5 overlap was {overlap}/5");
    }
}
