use crate::types::NO_BACKEND;

const OFFSET_SEED: u64 = 0xc3a5_c85c_97cb_3127;
const SKIP_SEED: u64 = 0xb492_b66f_be98_f273;

#[derive(Debug, Clone)]
pub struct Candidate {
    pub key: String,
    pub backend_idx: u32,
}

pub fn expand_weighted<F>(backends: &[(u32, u32)], key_for: F) -> Vec<Candidate>
where
    F: Fn(u32) -> String,
{
    let mut out = Vec::new();
    for (backend_idx, weight) in backends {
        let base = key_for(*backend_idx);
        for replica in 0..*weight {
            out.push(Candidate {
                key: format!("{base}#{replica}"),
                backend_idx: *backend_idx,
            });
        }
    }
    out
}

pub fn build_table(candidates: &[Candidate], size: usize) -> Vec<u32> {
    let mut table = vec![NO_BACKEND; size];
    if candidates.is_empty() || size == 0 {
        return table;
    }

    let permutation: Vec<(usize, usize)> = candidates
        .iter()
        .map(|c| {
            let offset = (hash64(&c.key, OFFSET_SEED) % size as u64) as usize;
            let skip = (hash64(&c.key, SKIP_SEED) % (size as u64 - 1)) as usize + 1;
            (offset, skip)
        })
        .collect();

    let mut next = vec![0usize; candidates.len()];
    let mut filled = 0usize;

    while filled < size {
        for i in 0..candidates.len() {
            let (offset, skip) = permutation[i];
            loop {
                let slot = (offset + next[i].wrapping_mul(skip)) % size;
                next[i] += 1;
                if table[slot] == NO_BACKEND {
                    table[slot] = candidates[i].backend_idx;
                    filled += 1;
                    break;
                }
            }
            if filled == size {
                break;
            }
        }
    }

    table
}

fn hash64(key: &str, seed: u64) -> u64 {
    let mut h = seed ^ 0xcbf2_9ce4_8422_2325;
    for byte in key.as_bytes() {
        h ^= *byte as u64;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h ^= h >> 33;
    h = h.wrapping_mul(0xff51_afd7_ed55_8ccd);
    h ^= h >> 33;
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidates(indices: &[u32]) -> Vec<Candidate> {
        indices
            .iter()
            .map(|i| Candidate {
                key: format!("10.0.0.{i}:8080#0"),
                backend_idx: *i,
            })
            .collect()
    }

    #[test]
    fn table_is_fully_populated() {
        let table = build_table(&candidates(&[0, 1, 2]), 4099);
        assert!(table.iter().all(|slot| *slot != NO_BACKEND));
    }

    #[test]
    fn empty_candidate_set_yields_no_backend() {
        let table = build_table(&[], 4099);
        assert!(table.iter().all(|slot| *slot == NO_BACKEND));
    }

    #[test]
    fn distribution_stays_within_one_percent() {
        let size = 4099;
        let table = build_table(&candidates(&[0, 1, 2, 3, 4]), size);
        let ideal = size as f64 / 5.0;
        for idx in 0..5u32 {
            let count = table.iter().filter(|slot| **slot == idx).count() as f64;
            let deviation = (count - ideal).abs() / ideal;
            assert!(deviation < 0.01, "backend {idx} deviates {deviation:.4}");
        }
    }

    #[test]
    fn removing_one_backend_disturbs_few_slots() {
        let size = 4099;
        let before = build_table(&candidates(&[0, 1, 2, 3, 4]), size);
        let after = build_table(&candidates(&[0, 1, 2, 4]), size);

        let moved = before
            .iter()
            .zip(&after)
            .filter(|(b, a)| **b != 3 && b != a)
            .count();

        let survivors = before.iter().filter(|slot| **slot != 3).count();
        let churn = moved as f64 / survivors as f64;
        assert!(churn < 0.02, "churn was {churn:.4}, expected under 2%");
    }

    #[test]
    fn weight_shifts_share_proportionally() {
        let size = 4099;
        let backends = [(0u32, 3u32), (1, 1)];
        let candidates = expand_weighted(&backends, |idx| format!("10.0.0.{idx}:8080"));
        let table = build_table(&candidates, size);

        let heavy = table.iter().filter(|slot| **slot == 0).count() as f64;
        let light = table.iter().filter(|slot| **slot == 1).count() as f64;
        let ratio = heavy / light;
        assert!(ratio > 2.8 && ratio < 3.2, "ratio was {ratio:.3}");
    }
}
