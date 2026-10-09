//! Группировка голосовых отпечатков в спикеров — спектральная кластеризация,
//! как в 3D-Speaker (авторы модели CAM++):
//!
//! 1. матрица косинусной похожести отпечатков;
//! 2. у каждой строки оставляем только самых похожих соседей — шум и выбросы
//!    перестают тянуть группы друг к другу;
//! 3. собственные векторы лапласиана этого графа; число спикеров (если не задано)
//!    — по самому большому скачку собственных чисел;
//! 4. k-средних в пространстве собственных векторов;
//! 5. мелкие группы (случайные шумы, кашель) отдаём ближайшим крупным,
//!    очень похожие группы — один и тот же человек — сливаем.

use nalgebra::{DMatrix, SymmetricEigen};

/// Больше стольких отпечатков в разложение не берём (время растёт как N³):
/// остальные приписываем к ближайшей найденной группе.
const MAX_SPECTRAL: usize = 800;
/// Сколько самых похожих соседей оставлять у каждого отпечатка (доля и минимум).
const PRUNE_KEEP: f64 = 0.022;
const PRUNE_MIN: usize = 6;
/// Меньше стольких отпечатков — не спикер, а случайность.
const MIN_CLUSTER: usize = 4;
/// Группы с такой похожестью центров — один человек.
const MERGE_COS: f32 = 0.8;
/// Совсем мало отпечатков — спектральный метод неустойчив, считаем попроще.
const MIN_SPECTRAL: usize = 20;

pub fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// Номер спикера для каждого отпечатка (отпечатки нормированы).
/// `speakers` — сколько людей, если известно; `max_speakers` — потолок для автоподбора.
pub fn cluster(embs: &[Vec<f32>], speakers: Option<usize>, max_speakers: usize) -> Vec<usize> {
    let n = embs.len();
    if n == 0 {
        return Vec::new();
    }
    if n == 1 || speakers == Some(1) {
        return vec![0; n];
    }

    // Длинная запись: раскладываем равномерную выборку, остальное — к ближайшему центру.
    let pick: Vec<usize> = if n > MAX_SPECTRAL {
        (0..MAX_SPECTRAL).map(|i| i * n / MAX_SPECTRAL).collect()
    } else {
        (0..n).collect()
    };
    let sample: Vec<&[f32]> = pick.iter().map(|&i| embs[i].as_slice()).collect();

    let mut labels = if sample.len() < MIN_SPECTRAL {
        agglomerative(&sample, speakers)
    } else {
        spectral(&sample, speakers, max_speakers)
    };
    labels = filter_minor(&sample, labels, speakers);
    if speakers.is_none() {
        labels = merge_similar(&sample, labels);
    }

    let labels = if pick.len() == n {
        labels
    } else {
        let cents = centroids(&sample, &labels);
        embs.iter().map(|e| nearest(&cents, e)).collect()
    };
    renumber(labels)
}

fn spectral(x: &[&[f32]], speakers: Option<usize>, max_speakers: usize) -> Vec<usize> {
    let n = x.len();
    // Похожесть и прореживание: у строки остаются только ближайшие соседи.
    let keep = ((PRUNE_KEEP * n as f64) as usize).max(PRUNE_MIN).min(n - 1);
    let mut a = DMatrix::<f64>::zeros(n, n);
    let mut row = vec![0f32; n];
    for i in 0..n {
        for j in 0..n {
            row[j] = if i == j { f32::NEG_INFINITY } else { dot(x[i], x[j]) };
        }
        let mut idx: Vec<usize> = (0..n).collect();
        idx.sort_by(|&p, &q| row[q].total_cmp(&row[p]));
        for &j in &idx[..keep] {
            a[(i, j)] = row[j].max(0.0) as f64;
        }
    }
    let a = (&a + a.transpose()) * 0.5;
    // Лапласиан графа L = D − A.
    let mut l = -a.clone();
    for i in 0..n {
        l[(i, i)] = a.row(i).sum();
    }
    let eig = SymmetricEigen::new(l);
    let mut order: Vec<usize> = (0..n).collect();
    order.sort_by(|&p, &q| eig.eigenvalues[p].total_cmp(&eig.eigenvalues[q]));

    let k = match speakers {
        Some(k) => k.min(n),
        None => {
            // Число спикеров — где собственные числа прыгают сильнее всего.
            let top = (max_speakers + 1).min(n);
            let vals: Vec<f64> = order[..top].iter().map(|&i| eig.eigenvalues[i]).collect();
            (1..vals.len())
                .max_by(|&p, &q| (vals[p] - vals[p - 1]).total_cmp(&(vals[q] - vals[q - 1])))
                .unwrap_or(1)
        }
    };
    if k <= 1 {
        return vec![0; n];
    }
    let points: Vec<Vec<f32>> =
        (0..n).map(|r| order[..k].iter().map(|&c| eig.eigenvectors[(r, c)] as f32).collect()).collect();
    kmeans(&points, k)
}

/// Попарное слияние по средней похожести — для совсем коротких записей.
fn agglomerative(x: &[&[f32]], speakers: Option<usize>) -> Vec<usize> {
    let n = x.len();
    let mut groups: Vec<Vec<usize>> = (0..n).map(|i| vec![i]).collect();
    let sim = |g: &[usize], h: &[usize]| {
        let s: f32 = g.iter().flat_map(|&i| h.iter().map(move |&j| dot(x[i], x[j]))).sum();
        s / (g.len() * h.len()) as f32
    };
    loop {
        let target = speakers.unwrap_or(1);
        if groups.len() <= target {
            break;
        }
        let mut best = (0, 1, f32::MIN);
        for i in 0..groups.len() {
            for j in i + 1..groups.len() {
                let s = sim(&groups[i], &groups[j]);
                if s > best.2 {
                    best = (i, j, s);
                }
            }
        }
        // Без заданного числа сливаем, пока группы похожи на одного человека.
        if speakers.is_none() && best.2 < 0.5 {
            break;
        }
        let g = groups.remove(best.1);
        groups[best.0].extend(g);
    }
    let mut labels = vec![0; n];
    for (c, g) in groups.iter().enumerate() {
        for &i in g {
            labels[i] = c;
        }
    }
    labels
}

/// k-средних с разумным стартом (k-means++) и несколькими попытками.
fn kmeans(points: &[Vec<f32>], k: usize) -> Vec<usize> {
    let n = points.len();
    let dist = |a: &[f32], b: &[f32]| a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum::<f32>();
    let mut rng = 0x9E37_79B9_7F4A_7C15u64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        (rng >> 11) as f64 / (1u64 << 53) as f64
    };
    let mut best: Option<(f32, Vec<usize>)> = None;
    for _ in 0..10 {
        let mut cents: Vec<Vec<f32>> = vec![points[(next() * n as f64) as usize % n].clone()];
        while cents.len() < k {
            let d: Vec<f32> = points.iter().map(|p| cents.iter().map(|c| dist(p, c)).fold(f32::MAX, f32::min)).collect();
            let total: f32 = d.iter().sum();
            let mut r = next() as f32 * total;
            let mut pick = n - 1;
            for (i, v) in d.iter().enumerate() {
                if r <= *v {
                    pick = i;
                    break;
                }
                r -= v;
            }
            cents.push(points[pick].clone());
        }
        let mut labels = vec![0; n];
        for _ in 0..100 {
            let mut changed = false;
            for (i, p) in points.iter().enumerate() {
                let c = (0..k).min_by(|&a, &b| dist(p, &cents[a]).total_cmp(&dist(p, &cents[b]))).unwrap();
                if labels[i] != c {
                    labels[i] = c;
                    changed = true;
                }
            }
            for (c, cent) in cents.iter_mut().enumerate() {
                let members: Vec<&Vec<f32>> = points.iter().zip(&labels).filter(|(_, &l)| l == c).map(|(p, _)| p).collect();
                if !members.is_empty() {
                    for (d, v) in cent.iter_mut().enumerate() {
                        *v = members.iter().map(|m| m[d]).sum::<f32>() / members.len() as f32;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        let inertia: f32 = points.iter().zip(&labels).map(|(p, &l)| dist(p, &cents[l])).sum();
        if best.as_ref().is_none_or(|b| inertia < b.0) {
            best = Some((inertia, labels));
        }
    }
    best.map(|b| b.1).unwrap_or_else(|| vec![0; n])
}

fn centroids(x: &[&[f32]], labels: &[usize]) -> Vec<Vec<f32>> {
    let k = labels.iter().max().map_or(0, |m| m + 1);
    let dim = x.first().map_or(0, |v| v.len());
    let mut c = vec![vec![0f32; dim]; k];
    for (v, &l) in x.iter().zip(labels) {
        c[l].iter_mut().zip(v.iter()).for_each(|(a, b)| *a += b);
    }
    c.iter_mut().for_each(|v| normalize(v));
    c
}

fn nearest(cents: &[Vec<f32>], e: &[f32]) -> usize {
    (0..cents.len())
        .filter(|&c| cents[c].iter().any(|v| *v != 0.0))
        .max_by(|&a, &b| dot(&cents[a], e).total_cmp(&dot(&cents[b], e)))
        .unwrap_or(0)
}

/// Группы меньше MIN_CLUSTER отпечатков — к ближайшей крупной.
fn filter_minor(x: &[&[f32]], labels: Vec<usize>, speakers: Option<usize>) -> Vec<usize> {
    let k = labels.iter().max().map_or(0, |m| m + 1);
    let size = |c: usize| labels.iter().filter(|&&l| l == c).count();
    let major: Vec<usize> = (0..k).filter(|&c| size(c) >= MIN_CLUSTER).collect();
    // Если крупных не осталось (или их меньше заданного), ничего не трогаем.
    if major.is_empty() || speakers.is_some_and(|s| major.len() < s.min(k)) {
        return labels;
    }
    let cents = centroids(x, &labels);
    let major_cents: Vec<Vec<f32>> = major.iter().map(|&c| cents[c].clone()).collect();
    labels
        .iter()
        .zip(x)
        .map(|(&l, v)| if major.contains(&l) { l } else { major[nearest(&major_cents, v)] })
        .collect()
}

/// Группы с почти одинаковыми центрами — один человек.
fn merge_similar(x: &[&[f32]], mut labels: Vec<usize>) -> Vec<usize> {
    loop {
        labels = renumber(labels);
        let cents = centroids(x, &labels);
        let mut best = (0, 0, MERGE_COS);
        for i in 0..cents.len() {
            for j in i + 1..cents.len() {
                let s = dot(&cents[i], &cents[j]);
                if s > best.2 {
                    best = (i, j, s);
                }
            }
        }
        if best.0 == best.1 {
            return labels;
        }
        labels.iter_mut().filter(|l| **l == best.1).for_each(|l| *l = best.0);
    }
}

/// Номера по порядку первого появления, без пропусков.
fn renumber(labels: Vec<usize>) -> Vec<usize> {
    let mut order: Vec<usize> = Vec::new();
    labels
        .into_iter()
        .map(|l| match order.iter().position(|&o| o == l) {
            Some(p) => p,
            None => {
                order.push(l);
                order.len() - 1
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Отпечатки нескольких «спикеров»: случайный центр + шум.
    fn voices(counts: &[usize], noise: f32, seed: u64) -> (Vec<Vec<f32>>, Vec<usize>) {
        let mut s = seed;
        let mut rnd = move || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            ((s >> 11) as f64 / (1u64 << 53) as f64) as f32 * 2.0 - 1.0
        };
        let dim = 64;
        let centers: Vec<Vec<f32>> = counts.iter().map(|_| (0..dim).map(|_| rnd()).collect()).collect();
        let mut embs = Vec::new();
        let mut truth = Vec::new();
        // Говорящие вперемешку, как в разговоре.
        let total: usize = counts.iter().sum();
        let mut left = counts.to_vec();
        for _ in 0..total {
            let alive: Vec<usize> = (0..counts.len()).filter(|&c| left[c] > 0).collect();
            let c = alive[(((rnd() + 1.0) / 2.0 * alive.len() as f32) as usize).min(alive.len() - 1)];
            left[c] -= 1;
            let mut v: Vec<f32> = centers[c].iter().map(|x| x + noise * rnd()).collect();
            normalize(&mut v);
            embs.push(v);
            truth.push(c);
        }
        (embs, truth)
    }

    fn agree(a: &[usize], b: &[usize]) -> bool {
        // Совпадение с точностью до перестановки номеров.
        let mut map = std::collections::HashMap::new();
        a.iter().zip(b).all(|(x, y)| *map.entry(*x).or_insert(*y) == *y)
            && map.values().collect::<std::collections::HashSet<_>>().len() == map.len()
    }

    #[test]
    fn finds_two_speakers_by_itself() {
        let (e, t) = voices(&[60, 40], 0.6, 1);
        let l = cluster(&e, None, 8);
        assert!(agree(&l, &t), "{l:?}");
    }

    #[test]
    fn finds_three_speakers_by_itself() {
        let (e, t) = voices(&[50, 30, 20], 0.6, 2);
        assert!(agree(&cluster(&e, None, 8), &t));
    }

    #[test]
    fn one_voice_stays_one_speaker() {
        let (e, _) = voices(&[80], 0.6, 3);
        assert_eq!(cluster(&e, None, 8).iter().max(), Some(&0));
    }

    #[test]
    fn outlier_does_not_steal_a_speaker() {
        // Заданы двое, плюс один странный отпечаток (кашель): он не должен
        // стать «вторым спикером», забрав у настоящего второго всю речь.
        let (mut e, mut t) = voices(&[40, 40], 0.6, 4);
        let mut odd: Vec<f32> = (0..64).map(|i| if i % 2 == 0 { 1.0 } else { -1.0 }).collect();
        normalize(&mut odd);
        e.push(odd);
        t.push(0);
        let l = cluster(&e, Some(2), 8);
        assert!(agree(&l[..80], &t[..80]));
    }

    #[test]
    fn long_recording_uses_sample() {
        let (e, t) = voices(&[900, 700], 0.6, 5);
        assert!(agree(&cluster(&e, None, 8), &t));
    }

    #[test]
    fn short_recording() {
        let (e, t) = voices(&[6, 5], 0.4, 6);
        assert!(agree(&cluster(&e, Some(2), 8), &t));
        assert!(agree(&cluster(&e, None, 8), &t));
    }
}
