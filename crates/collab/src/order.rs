//! Fractional order keys: the stacking order of siblings without a shared array.
//!
//! Each object carries a key; siblings paint in key order (ties broken by id). A key is a base-62
//! fraction `0.d1d2d3…` written as its digits, never ending in the zero digit, so plain string
//! comparison is numeric comparison. Moving an object rewrites only its own key, and two people
//! reordering at once can't duplicate or lose an object, as they could with a shared list.

const DIGITS: &[u8; 62] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
const BASE: usize = 62;

fn digit(c: u8) -> usize {
    DIGITS.iter().position(|d| *d == c).unwrap_or(0)
}

fn valid(key: &str) -> bool {
    !key.is_empty() && key.bytes().all(|c| DIGITS.contains(&c)) && !key.ends_with('0')
}

/// A key strictly between `a` and `b` (`None`: the open ends). Invalid or misordered bounds (keys
/// from a peer are untrusted) are ignored rather than trusted, so the result is always a valid key.
pub fn between(a: Option<&str>, b: Option<&str>) -> String {
    let a = a.filter(|k| valid(k)).unwrap_or("");
    let b = b.filter(|k| valid(k) && *k > a);
    let mut out = String::new();
    midpoint(a.as_bytes(), b.map(str::as_bytes), &mut out);
    out
}

fn midpoint(a: &[u8], b: Option<&[u8]>, out: &mut String) {
    let mut a = a;
    let mut b = b;
    // Bounded by the bounds' length: each round consumes a digit of one of them.
    loop {
        if let Some(bb) = b {
            let n = (0..bb.len()).take_while(|i| a.get(*i).copied().unwrap_or(b'0') == bb[*i]).count();
            if n > 0 {
                out.extend(bb[..n].iter().map(|c| *c as char));
                a = a.get(n..).unwrap_or(&[]);
                b = Some(&bb[n..]);
                continue;
            }
        }
        let da = a.first().map_or(0, |c| digit(*c));
        let db = b.and_then(|bb| bb.first()).map_or(BASE, |c| digit(*c));
        if db > da + 1 {
            out.push(DIGITS[(da + db).div_ceil(2)] as char);
            return;
        }
        if let Some(bb) = b
            && bb.len() > 1
        {
            out.push(bb[0] as char);
            return;
        }
        out.push(DIGITS[da] as char);
        a = a.get(1..).unwrap_or(&[]);
        b = None;
    }
}

/// `n` increasing keys strictly between `a` and `b`, by halving the gap so they stay short (about
/// `log62(n)` digits longer than the bounds) however many are inserted at one spot.
pub fn n_between(a: Option<&str>, b: Option<&str>, n: usize) -> Vec<String> {
    let mut out = Vec::with_capacity(n);
    fill(a.map(str::to_string), b.map(str::to_string), n, &mut out);
    out
}

fn fill(a: Option<String>, b: Option<String>, n: usize, out: &mut Vec<String>) {
    if n == 0 {
        return;
    }
    let mid = between(a.as_deref(), b.as_deref());
    let left = n / 2;
    fill(a, Some(mid.clone()), left, out);
    out.push(mid.clone());
    fill(Some(mid), b, n - left - 1, out);
}

/// New keys for a child list. `old[i]` is the key child `i` already has (`None`: new here). The
/// longest run of children whose old keys are already increasing keeps its keys; the rest get
/// fresh keys between their kept neighbours. → the key for every child, in list order.
pub fn assign(old: &[Option<&str>]) -> Vec<String> {
    let keep = longest_increasing(old);
    let mut out: Vec<String> = Vec::with_capacity(old.len());
    let mut i = 0;
    while i < old.len() {
        if keep.get(i).copied().unwrap_or(false)
            && let Some(Some(k)) = old.get(i)
        {
            out.push((*k).to_string());
            i += 1;
            continue;
        }
        // A run of children needing keys: between the previous key and the next kept one.
        let start = i;
        while i < old.len() && !keep.get(i).copied().unwrap_or(false) {
            i += 1;
        }
        let next = if i < old.len() { old.get(i).copied().flatten() } else { None };
        let prev = out.last().cloned();
        out.extend(n_between(prev.as_deref(), next, i - start));
    }
    out
}

/// Which entries form a longest strictly increasing subsequence of the valid keys (patience
/// sorting, O(n log n)).
fn longest_increasing(keys: &[Option<&str>]) -> Vec<bool> {
    let mut tails: Vec<usize> = vec![];
    let mut prev: Vec<Option<usize>> = vec![None; keys.len()];
    for (i, k) in keys.iter().enumerate() {
        let Some(k) = k.filter(|k| valid(k)) else { continue };
        let pos = tails.partition_point(|&t| keys.get(t).copied().flatten().is_some_and(|tk| tk < k));
        if pos > 0 {
            prev[i] = tails.get(pos - 1).copied();
        }
        if pos == tails.len() {
            tails.push(i);
        } else {
            tails[pos] = i;
        }
    }
    let mut keep = vec![false; keys.len()];
    let mut cur = tails.last().copied();
    while let Some(i) = cur {
        keep[i] = true;
        cur = prev[i];
    }
    keep
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn between_orders() {
        let a = between(None, None);
        let b = between(Some(&a), None);
        let c = between(Some(&a), Some(&b));
        assert!(a < c && c < b, "{a} {c} {b}");
        assert!(valid(&a) && valid(&b) && valid(&c));
    }

    #[test]
    fn many_appends_stay_short() {
        let keys = n_between(None, None, 10_000);
        assert!(keys.windows(2).all(|w| w[0] < w[1]));
        assert!(keys.iter().all(|k| k.len() <= 6), "{:?}", keys.iter().map(String::len).max());
    }

    #[test]
    fn assign_keeps_unmoved() {
        let old = [Some("1"), Some("2"), None, Some("3"), Some("4")];
        let new = assign(&old);
        assert_eq!(new[0], "1");
        assert_eq!(new[1], "2");
        assert_eq!(new[3], "3");
        assert!(new[1].as_str() < new[2].as_str() && new[2].as_str() < "3");
        // Moving the last one to the front only rekeys it.
        let moved = assign(&[Some("4"), Some("1"), Some("2"), Some("3")]);
        assert_eq!(&moved[1..], &["1", "2", "3"]);
        assert!(moved[0].as_str() < "1");
    }

    #[test]
    fn garbage_bounds_are_ignored() {
        let k = between(Some("zz0"), Some("!!"));
        assert!(valid(&k));
        let k = between(Some("V"), Some("A"));
        assert!(valid(&k) && k.as_str() > "V");
    }

    proptest! {
        #[test]
        fn assign_is_increasing(old in proptest::collection::vec(proptest::option::of("[0-9a-zA-Z]{1,4}"), 0..40)) {
            let refs: Vec<Option<&str>> = old.iter().map(|o| o.as_deref()).collect();
            let keys = assign(&refs);
            prop_assert_eq!(keys.len(), old.len());
            prop_assert!(keys.windows(2).all(|w| w[0] < w[1]), "{:?} -> {:?}", old, keys);
            prop_assert!(keys.iter().all(|k| valid(k)));
        }

        #[test]
        fn between_is_between(a in "[0-9a-zA-Z]{0,5}[1-9a-zA-Z]", b in "[0-9a-zA-Z]{0,5}[1-9a-zA-Z]") {
            let (lo, hi) = if a < b { (a, b) } else { (b, a) };
            prop_assume!(lo != hi);
            let k = between(Some(&lo), Some(&hi));
            prop_assert!(lo < k && k < hi, "{} {} {}", lo, k, hi);
            prop_assert!(valid(&k));
        }
    }
}
