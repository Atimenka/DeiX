//! DAG (Directed Acyclic Graph) for Nano-Core boot dependency tracking.
//!
//! The DAG tracks which Nano-Threads can run in parallel based on their
//! `requires` dependencies. A process can start only when all its
//! dependencies have completed successfully.
//!
//! No external crates — simple adjacency list with topological sort.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

/// A directed graph representing process dependencies.
///
/// Edges go from dependency → dependent (A must complete before B can start).
#[derive(Clone)]
pub struct Dag {
    /// name → list of processes that depend on it (A → B means A must finish before B)
    dependents: BTreeMap<String, Vec<String>>,
    /// name → list of dependencies (B → A means B requires A)
    dependencies: BTreeMap<String, Vec<String>>,
    /// All known process names.
    nodes: Vec<String>,
}

impl Dag {
    pub fn new() -> Self {
        Dag {
            dependents: BTreeMap::new(),
            dependencies: BTreeMap::new(),
            nodes: Vec::new(),
        }
    }

    /// Add a process node with its dependencies.
    pub fn add_node(&mut self, name: &str, requires: &[String]) {
        if !self.nodes.iter().any(|n| n == name) {
            self.nodes.push(name.to_string());
        }
        self.dependencies.entry(name.to_string()).or_insert_with(Vec::new);
        for dep in requires {
            self.dependencies
                .entry(name.to_string())
                .or_default()
                .push(dep.clone());
            self.dependents
                .entry(dep.clone())
                .or_default()
                .push(name.to_string());
            // Ensure dependency node exists even if it has no manifest yet.
            if !self.nodes.iter().any(|n| n == dep) {
                self.nodes.push(dep.clone());
            }
        }
    }

    /// Get processes that depend on `name` (must wait for `name` to finish).
    pub fn dependents(&self, name: &str) -> &[String] {
        self.dependents.get(name).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Get dependencies of `name` (must finish before `name` starts).
    pub fn dependencies(&self, name: &str) -> &[String] {
        self.dependencies.get(name).map(|v| v.as_slice()).unwrap_or(&[])
    }

    /// Get all process names.
    pub fn nodes(&self) -> &[String] {
        &self.nodes
    }

    /// Are all dependencies of `name` satisfied (i.e., in `completed`)?
    pub fn ready(&self, name: &str, completed: &[&str]) -> bool {
        let deps = self.dependencies(name);
        deps.iter().all(|d| completed.contains(&d.as_str()))
    }

    /// Get all processes that are ready to run given the completed set.
    /// Returns names in priority-independent order.
    pub fn ready_set(&self, completed: &[&str]) -> Vec<String> {
        self.nodes
            .iter()
            .filter(|name| {
                // Skip already-completed
                if completed.contains(&name.as_str()) {
                    return false;
                }
                // Check all deps are completed
                self.ready(name, completed)
            })
            .cloned()
            .collect()
    }

    /// Topological sort (Kahn's algorithm). Returns error if cycle detected.
    pub fn topological_sort(&self) -> Result<Vec<String>, String> {
        // in_degree = number of dependencies that must complete first.
        // Edge A → B means A must finish before B. B has in_degree from A.
        // in_degree[B] = len(B.deps)
        let mut in_degree: BTreeMap<String, usize> = BTreeMap::new();
        for (node, deps) in &self.dependencies {
            in_degree.insert(node.clone(), deps.len());
        }
        // Ensure all nodes have an entry (nodes with no deps map to 0).
        for node in &self.nodes {
            in_degree.entry(node.clone()).or_insert(0);
        }

        // Start with nodes that have no dependencies.
        let mut queue: Vec<String> = in_degree
            .iter()
            .filter(|(_, &deg)| deg == 0)
            .map(|(n, _)| n.clone())
            .collect();

        let mut result = Vec::with_capacity(self.nodes.len());
        let mut processed = 0;

        while let Some(node) = queue.pop() {
            result.push(node.clone());
            processed += 1;
            // For each process that depends on this node, reduce its in_degree.
            for dependent in self.dependents(&node) {
                if let Some(e) = in_degree.get_mut(dependent) {
                    *e -= 1;
                    if *e == 0 {
                        queue.push(dependent.clone());
                    }
                }
            }
        }

        if processed != self.nodes.len() {
            Err("cycle detected in DAG".to_string())
        } else {
            Ok(result)
        }
    }

    /// Compute the parallelism level at each step of the topological order.
    /// Returns a vector of "waves" — each wave contains processes that can
    /// run simultaneously.
    pub fn waves(&self) -> Vec<Vec<String>> {
        let sorted = match self.topological_sort() {
            Ok(s) => s,
            Err(_) => return Vec::new(),
        };
        let mut waves: Vec<Vec<String>> = Vec::new();
        let mut completed: Vec<String> = Vec::new();

        while completed.len() < sorted.len() {
            let wave: Vec<String> = sorted
                .iter()
                .filter(|name| {
                    !completed.contains(name) && self.ready(name, &completed.iter().map(|s| s.as_str()).collect::<Vec<_>>())
                })
                .cloned()
                .collect();
            if wave.is_empty() {
                break;
            }
            waves.push(wave);
            for w in &wave {
                completed.push(w.clone());
            }
        }
        waves
    }
}

impl core::default::Default for Dag {
    fn default() -> Self {
        Self::new()
    }
}
