//! `cargo xtask netcheck`: which network-capable crates the batch path can reach.
//!
//! The batch folder job runs `pdfcraft-cli`, which drives `pdfcraft-automation` over
//! `pdfcraft-engine`; none of them may open a socket. That promise is only as strong as the
//! dependency graph behind those three packages, so this gate walks the resolved graph recorded
//! in `Cargo.lock` and asks, for each of them and each crate that can open a socket, whether that
//! crate is reachable from it. The answer is compared with a reviewed baseline in
//! `xtask/baselines/network-crates.json`: a reachable pair the baseline does not list fails.
//!
//! A lockfile records the resolved dependencies of a package without their kind and without a
//! target: normal, build and development edges look the same, and a dependency only one platform
//! uses is listed beside the rest. The answer here is therefore a superset of what a normal-level,
//! per-target walk would give. It may name a crate that is only a build or a development
//! dependency, or one that only another platform uses; it does not miss one, so it errs toward
//! failing and never toward passing. `--update-baseline` rewrites the baseline; review that diff
//! line by line.
//!
//! The walk is in process, over the lockfile text: it starts no other program and reads no
//! network. This gate is local on purpose, not part of `cargo xtask ci`, which would be a
//! workflow change.

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::path::Path;

use anyhow::{Context, bail};
use serde::{Deserialize, Serialize};

use crate::gates::root;

/// The packages the batch path is built from.
const PACKAGES: &[&str] = &["pdfcraft-cli", "pdfcraft-automation", "pdfcraft-engine"];

/// Crates that can open a socket: HTTP clients, TLS stacks, async runtimes and libcurl bindings.
const NETWORK_CRATES: &[&str] = &["curl", "hyper", "isahc", "minreq", "native-tls", "reqwest", "tokio", "ureq"];

/// The resolved dependency graph of the workspace, relative to the repository root.
const LOCKFILE: &str = "Cargo.lock";

/// The reviewed baseline, relative to the repository root.
const BASELINE: &str = "xtask/baselines/network-crates.json";

/// The most packages one walk will visit. A lock file has a few hundred packages; the cap stops a
/// damaged file from turning the walk into a long loop.
const MAX_VISITED: usize = 500_000;

/// What the lock file says about one package and one network-capable crate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Row {
    /// The package whose dependencies were walked.
    #[serde(rename = "crate")]
    package: String,
    /// The network-capable crate asked about.
    name: String,
    /// Whether the package reaches the crate in the lock file graph.
    reachable: bool,
    /// The chain from the package to the crate, ` -> ` separated; empty when unreachable.
    #[serde(default)]
    path: String,
}

/// The first word of a dependency entry: `"name"`, `"name 1.2.3"` and `"name 1.2.3 (source)"`
/// all name the same package.
fn entry_name(entry: &str) -> &str {
    entry.split_whitespace().next().unwrap_or_default()
}

/// The package a dependency line names, when the line is one.
fn dependency_name(line: &str) -> Option<&str> {
    let quoted = line.strip_prefix('"')?;
    let end = quoted.find('"')?;
    let name = entry_name(&quoted[..end]);
    (!name.is_empty()).then_some(name)
}

/// The names in a `dependencies = ["a", "b"]` line, that is, the text between its brackets.
fn inline_names(value: &str) -> Vec<&str> {
    value.split(',').filter_map(|item| dependency_name(item.trim())).collect()
}

/// One edge of the graph: `package` depends on `dependency`.
fn add_edge(graph: &mut BTreeMap<String, BTreeSet<String>>, package: &str, dependency: &str) {
    graph.entry(package.to_owned()).or_default().insert(dependency.to_owned());
}

/// The dependency graph recorded by a lock file, keyed by package name.
///
/// Edges are by name: a lock file can hold two versions of one crate, and the walk only asks
/// whether a name can be reached at all. A file that names no package is an error rather than an
/// empty graph, so a format change fails loudly instead of reporting nothing reachable.
fn parse_lockfile(text: &str) -> anyhow::Result<BTreeMap<String, BTreeSet<String>>> {
    let mut graph: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut current: Option<String> = None;
    let mut in_list = false;
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if in_list {
            if line.starts_with(']') {
                in_list = false;
            } else if let Some(name) = dependency_name(line)
                && let Some(package) = current.as_deref()
            {
                add_edge(&mut graph, package, name);
            }
            continue;
        }
        if line.starts_with("[[") {
            current = None;
            continue;
        }
        if let Some(name) = line.strip_prefix("name = \"").and_then(|rest| rest.strip_suffix('"')) {
            graph.entry(name.to_owned()).or_default();
            current = Some(name.to_owned());
            continue;
        }
        if let Some(value) = line.strip_prefix("dependencies = [") {
            match value.strip_suffix(']') {
                Some(inner) => {
                    for name in inline_names(inner) {
                        if let Some(package) = current.as_deref() {
                            add_edge(&mut graph, package, name);
                        }
                    }
                }
                None => in_list = true,
            }
        }
    }
    if graph.is_empty() {
        bail!("{LOCKFILE} names no package; the lock file format changed or the file is empty");
    }
    Ok(graph)
}

/// The shortest chain from `from` to `to` in the graph, both ends included.
fn shortest_path(graph: &BTreeMap<String, BTreeSet<String>>, from: &str, to: &str) -> Option<Vec<String>> {
    let mut parents: BTreeMap<String, String> = BTreeMap::new();
    let mut queue: VecDeque<String> = VecDeque::from([from.to_owned()]);
    let mut visited = 0usize;
    while let Some(node) = queue.pop_front() {
        visited = visited.saturating_add(1);
        if visited > MAX_VISITED {
            return None;
        }
        if node == to {
            return Some(walk_back(&parents, node));
        }
        let Some(next) = graph.get(&node) else { continue };
        for dependency in next {
            if dependency != from && !parents.contains_key(dependency.as_str()) {
                parents.insert(dependency.clone(), node.clone());
                queue.push_back(dependency.clone());
            }
        }
    }
    None
}

/// The chain from the start of the walk to `node`, following the discovery parents.
fn walk_back(parents: &BTreeMap<String, String>, node: String) -> Vec<String> {
    let mut chain = vec![node.clone()];
    let mut current = node;
    while let Some(parent) = parents.get(&current) {
        chain.push(parent.clone());
        current = parent.clone();
    }
    chain.reverse();
    chain
}

/// One row per package and network-capable crate.
fn rows(graph: &BTreeMap<String, BTreeSet<String>>) -> Vec<Row> {
    let mut rows = Vec::with_capacity(PACKAGES.len().saturating_mul(NETWORK_CRATES.len()));
    for package in PACKAGES {
        for name in NETWORK_CRATES {
            let path = shortest_path(graph, package, name);
            let path = path.map(|chain| chain.join(" -> "));
            let reachable = path.is_some();
            let row = Row { package: (*package).to_owned(), name: (*name).to_owned(), reachable, path: path.unwrap_or_default() };
            rows.push(row);
        }
    }
    rows
}

/// Write the reviewed baseline: the reachable pairs only, so a reader sees exactly the accepted
/// answers and nothing else.
fn write_baseline(path: &Path, rows: &[Row]) -> anyhow::Result<()> {
    let reachable: Vec<&Row> = rows.iter().filter(|r| r.reachable).collect();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(&reachable)? + "\n";
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    println!("baseline updated: {} rows in {BASELINE}", reachable.len());
    Ok(())
}

fn read_baseline(path: &Path) -> anyhow::Result<Vec<Row>> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("{} is missing: run `cargo xtask netcheck --update-baseline`", path.display()))?;
    serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))
}

/// The reachable pairs of a row list, for set comparison.
fn pairs(rows: &[Row]) -> BTreeSet<(&str, &str)> {
    rows.iter().filter(|r| r.reachable).map(|r| (r.package.as_str(), r.name.as_str())).collect()
}

/// Compare the current answers with the reviewed baseline: a reachable pair the baseline does not
/// list fails, and a pair that stopped being reachable or moved to another path is reported for
/// the next baseline update.
fn check_rows(rows: &[Row], reviewed: &[Row]) -> anyhow::Result<()> {
    let listed = pairs(reviewed);
    let found = pairs(rows);
    for row in rows.iter().filter(|r| r.reachable) {
        if let Some(before) = reviewed.iter().find(|b| b.reachable && b.package == row.package && b.name == row.name)
            && before.path != row.path
        {
            println!("path changed for {} -> {}: {} (was {})", row.package, row.name, row.path, before.path);
        }
    }
    let gone: Vec<String> = listed.difference(&found).map(|(package, name)| format!("{package} -> {name}")).collect();
    if !gone.is_empty() {
        println!("no longer reachable (run --update-baseline to record it): {}", gone.join("; "));
    }
    let new: Vec<String> = found.difference(&listed).map(|(package, name)| format!("{package} -> {name}")).collect();
    if new.is_empty() {
        println!("OK: all {} reachable pairs are in the reviewed baseline", found.len());
        return Ok(());
    }
    for pair in &new {
        println!("  - {pair}");
    }
    bail!("{} reachable network crate pair(s) are not in {BASELINE}; review them, then run `cargo xtask netcheck --update-baseline`", new.len())
}

pub fn run(args: &[String]) -> anyhow::Result<()> {
    let update = args.iter().any(|a| a == "--update-baseline");
    let lockfile = root().join(LOCKFILE);
    let text = std::fs::read_to_string(&lockfile).with_context(|| format!("reading {}", lockfile.display()))?;
    let graph = parse_lockfile(&text)?;
    if let Some(package) = PACKAGES.iter().find(|name| !graph.contains_key(**name)) {
        bail!("{package} is not in {LOCKFILE}; is it still a workspace member?");
    }
    let rows = rows(&graph);
    let reachable = rows.iter().filter(|r| r.reachable).count();
    println!("netcheck: {} packages x {} network-capable crates; {reachable} reachable", PACKAGES.len(), NETWORK_CRATES.len());
    for row in rows.iter().filter(|r| r.reachable) {
        println!("  {} -> {}: {}", row.package, row.name, row.path);
    }
    let path = root().join(BASELINE);
    let reviewed = if update {
        write_baseline(&path, &rows)?;
        rows.clone()
    } else {
        read_baseline(&path)?
    };
    check_rows(&rows, &reviewed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lock file with a block list, an inline list, a versioned entry and a patch block.
    const SAMPLE: &str = r#"
version = 4

[[package]]
name = "root"
version = "0.1.0"
dependencies = [
 "middle",
 "serde 1.0.0",
]

[[package]]
name = "middle"
version = "0.1.0"
source = "registry+https://example.invalid/index"
dependencies = [
 "leaf",
]

[[package]]
name = "leaf"
version = "0.1.0"

[[package]]
name = "empty"
version = "0.1.0"
dependencies = []

[[patch.unused]]
name = "spare"
version = "0.2.0"

[[package]]
name = "tail"
version = "0.1.0"
dependencies = ["leaf"]
"#;

    #[test]
    fn lockfile_dependency_lists_become_edges() {
        let graph = parse_lockfile(SAMPLE).expect("the sample parses");
        assert_eq!(graph.get("root"), Some(&BTreeSet::from(["middle".to_string(), "serde".to_string()])));
        assert_eq!(graph.get("middle"), Some(&BTreeSet::from(["leaf".to_string()])));
        assert_eq!(graph.get("tail"), Some(&BTreeSet::from(["leaf".to_string()])), "an inline list is read too");
        assert_eq!(graph.get("empty"), Some(&BTreeSet::new()));
        assert!(graph.contains_key("leaf"), "a package with no dependency list is still a node");
        assert!(graph.contains_key("spare"), "a patch block names a package too");
    }

    #[test]
    fn shortest_path_names_the_chain_and_follows_dependencies_only() {
        let graph = parse_lockfile(SAMPLE).expect("the sample parses");
        assert_eq!(shortest_path(&graph, "root", "leaf"), Some(vec!["root".to_string(), "middle".to_string(), "leaf".to_string()]));
        assert_eq!(shortest_path(&graph, "leaf", "root"), None);
        assert_eq!(shortest_path(&graph, "root", "nowhere"), None);
        assert_eq!(shortest_path(&graph, "root", "root"), Some(vec!["root".to_string()]));
    }

    #[test]
    fn a_lockfile_without_packages_is_an_error() {
        let error = parse_lockfile("version = 4\n").expect_err("an empty graph must not be read as an answer");
        assert!(error.to_string().contains("names no package"), "{error}");
    }

    #[test]
    fn committed_baseline_lists_the_reachable_pairs_of_the_lockfile_graph() {
        let text = std::fs::read_to_string(root().join(LOCKFILE)).expect("Cargo.lock is committed");
        let graph = parse_lockfile(&text).expect("Cargo.lock parses");
        let found = rows(&graph);
        let reviewed = read_baseline(&root().join(BASELINE)).expect("the baseline is committed");
        let list = |rows: &[Row]| -> Vec<String> {
            rows.iter().filter(|r| r.reachable).map(|r| format!("{} -> {}: {}", r.package, r.name, r.path)).collect()
        };
        assert_eq!(list(&reviewed), list(&found), "{BASELINE} must list exactly the pairs the lock file graph gives");
    }
}
