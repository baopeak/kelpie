use super::rank::rank_on;
use super::*;

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("kelpie-search-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn paths(hits: &[QuickOpenHit]) -> Vec<&str> {
    hits.iter().map(|h| h.path.as_str()).collect()
}

#[test]
fn index_lists_folders_with_a_trailing_slash() {
    let root = scratch("dirs");
    std::fs::create_dir_all(root.join("src/ui")).unwrap();
    std::fs::create_dir_all(root.join("docs")).unwrap();
    std::fs::write(root.join("src/ui/panel.rs"), "x").unwrap();
    std::fs::write(root.join("README.md"), "x").unwrap();
    let mut index = walk_index(&root);
    index.sort();
    assert_eq!(index, ["README.md", "docs/", "src/", "src/ui/", "src/ui/panel.rs"]);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn folders_are_found_only_when_asked_for() {
    let files: Vec<String> = ["src/", "src/ui/", "src/ui/panel.rs", "docs/", "docs/ui-guide.md"].map(String::from).to_vec();
    let with = rank(&files, "ui", 10, true);
    assert!(paths(&with).contains(&"src/ui/"));
    assert!(with.iter().find(|h| h.path == "src/ui/").unwrap().is_dir);
    assert!(!with.iter().find(|h| h.path == "src/ui/panel.rs").unwrap().is_dir);
    let without = rank(&files, "ui", 10, false);
    assert!(without.iter().all(|h| !h.path.ends_with('/') && !h.is_dir));
    // No query: the first entries, still without folders unless asked.
    assert_eq!(paths(&rank(&files, "", 10, false)), ["src/ui/panel.rs", "docs/ui-guide.md"]);
    assert_eq!(rank(&files, "", 2, true).len(), 2);
}

#[test]
fn a_folder_name_beats_a_deep_file_that_merely_contains_it() {
    let files: Vec<String> = ["app/src/components/", "app/src/components/right-panel.tsx", "src-tauri/src/search.rs"].map(String::from).to_vec();
    let hits = rank(&files, "components", 5, true);
    assert_eq!(hits[0].path, "app/src/components/");
    assert!(!hits[0].indices.is_empty());
}

#[test]
fn the_filename_bonus_ranks_a_name_match_above_a_scattered_path_match() {
    let files: Vec<String> = ["domain/model/x.rs", "src/main.rs", "mail/arch/index.rs"].map(String::from).to_vec();
    assert_eq!(rank(&files, "main", 3, false)[0].path, "src/main.rs");
}

/// A big index is scored by several workers: the result must not depend on how many.
#[test]
fn the_result_does_not_depend_on_the_number_of_workers() {
    let files: Vec<String> = (0..20_000).map(|i| format!("pkg-{}/src/mod-{}/file-{}.ts{}", i % 37, i % 101, i, if i % 50 == 0 { "/" } else { "" })).collect();
    for query in ["file", "pkg-3 mod-1", "src/file-19", "ts", "zzz"] {
        for dirs in [true, false] {
            let one = rank_on(&files, query, 50, dirs, 1);
            for workers in [2, 3, 8] {
                let many = rank_on(&files, query, 50, dirs, workers);
                assert_eq!(one.iter().map(|h| (&h.path, h.score)).collect::<Vec<_>>(), many.iter().map(|h| (&h.path, h.score)).collect::<Vec<_>>(), "{query:?} dirs={dirs} workers={workers}");
            }
        }
    }
}

#[test]
fn remote_file_lists_gain_their_parent_folders_once() {
    let files: Vec<String> = ["a/b/c.txt", "a/b/d.txt", "a/e.txt", "top.txt"].map(String::from).to_vec();
    let mut all = with_parent_dirs(files);
    all.sort();
    assert_eq!(all, ["a/", "a/b/", "a/b/c.txt", "a/b/d.txt", "a/e.txt", "top.txt"]);
}

/// Fuzzy search on a big tree: `KELPIE_BENCH_TREE=<dir>` names an empty scratch folder to fill
/// with 50 000 files (left in place for the caller to delete).
/// `cargo test --release -p kelpie fuzzy_p95 -- --ignored --nocapture`
#[test]
#[ignore]
fn fuzzy_p95_on_fifty_thousand_files() {
    let root = std::path::PathBuf::from(std::env::var("KELPIE_BENCH_TREE").expect("KELPIE_BENCH_TREE"));
    let mut made = 0;
    'fill: for a in 0..25 {
        for b in 0..20 {
            let dir = root.join(format!("module-{a}")).join(format!("component-{b}")).join("src");
            std::fs::create_dir_all(&dir).unwrap();
            for c in 0..100 {
                std::fs::write(dir.join(format!("file-{c}-{}.ts", ["view", "store", "util", "model"][c % 4])), "x").unwrap();
                made += 1;
                if made == 50_000 {
                    break 'fill;
                }
            }
        }
    }
    assert_eq!(made, 50_000);
    let started = std::time::Instant::now();
    let index = walk_index(&root);
    println!("index of {} entries built in {:?}", index.len(), started.elapsed());
    let queries = ["file", "view", "mod12 comp store", "src/file-7", "component-19 util", "zzz", "m3c4", "model 99", "ts", "module-24/component"];
    let mut times = Vec::new();
    for round in 0..20 {
        for q in queries {
            let t = std::time::Instant::now();
            let hits = rank(&index, q, 50, true);
            times.push(t.elapsed().as_secs_f64() * 1000.0);
            if round == 0 {
                println!("{q:?}: {} hits", hits.len());
            }
        }
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p95 = times[times.len() * 95 / 100];
    println!("fuzzy over {} entries: p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms", index.len(), times[times.len() / 2], p95, times[times.len() - 1]);
    // An unoptimised build is an order of magnitude slower: only the shipped one is held to the budget.
    if cfg!(debug_assertions) {
        println!("debug build: p95 not asserted");
    } else {
        assert!(p95 <= 30.0, "p95 {p95:.1} ms");
    }
}
