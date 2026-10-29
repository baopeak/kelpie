//! Process-tree helpers shared by the daemon (which lists and stops what a session runs) and the
//! app (which sums the memory of the daemon's processes).

use std::collections::{HashMap, HashSet};
use std::hash::Hash;

/// Parent -> children from `(pid, parent pid, start time)` rows. A "parent" that started after its
/// child is a newer process that reused a dead parent's PID, not the real parent, so it is not one.
pub fn child_map<K: Copy + Eq + Hash>(rows: &[(K, Option<K>, u64)]) -> HashMap<K, Vec<K>> {
    let start: HashMap<K, u64> = rows.iter().map(|(pid, _, start)| (*pid, *start)).collect();
    let mut children: HashMap<K, Vec<K>> = HashMap::new();
    for (pid, parent, started) in rows {
        if let Some(parent) = parent.filter(|p| start.get(p).is_some_and(|s| s <= started)) {
            children.entry(parent).or_default().push(*pid);
        }
    }
    children
}

/// Every descendant of `root` (not `root` itself), parents before their children. The graph can
/// contain cycles (Windows reuses the PIDs of dead parents), so each process appears once.
pub fn descendants<K: Copy + Eq + Hash>(children: &HashMap<K, Vec<K>>, root: K) -> Vec<K> {
    let mut out = Vec::new();
    let mut seen = HashSet::from([root]);
    let mut stack: Vec<K> = children.get(&root).map(|c| c.iter().rev().copied().collect()).unwrap_or_default();
    while let Some(pid) = stack.pop() {
        if !seen.insert(pid) {
            continue;
        }
        out.push(pid);
        if let Some(c) = children.get(&pid) {
            stack.extend(c.iter().rev());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descendants_survive_cycles() {
        let children = HashMap::from([(1u32, vec![2]), (2, vec![3]), (3, vec![1, 2])]);
        assert_eq!(descendants(&children, 1), vec![2, 3]);
        assert!(descendants(&children, 9).is_empty());
    }

    #[test]
    fn a_parent_that_started_later_is_a_reused_pid_not_a_parent() {
        // 5 is "parent" of 6, but started after it: PID 5 was reused.
        let rows = [(1u32, None, 10), (5, Some(1), 50), (6, Some(5), 20), (7, Some(1), 30)];
        let children = child_map(&rows);
        assert_eq!(children.get(&1).map(|c| c.len()), Some(2));
        assert!(!children.contains_key(&5));
        assert_eq!(descendants(&children, 1), vec![5, 7]);
    }
}
