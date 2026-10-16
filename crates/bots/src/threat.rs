//! Where enemies come from. Every player knows the map: which end the other
//! team starts at and the ways from there. The bots note where each team
//! stands during the buy freeze and, once a round, spread out over the
//! navigation graph from the enemy start; walking that spread backwards from
//! a bot gives the way an enemy would arrive at it, so it watches the door or
//! corner they would come through instead of a wall or its own spawn.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use crate::nav::{self, NavGraph};

/// How far back along the enemy's way in the watched point sits.
const APPROACH_IN: f32 = 600.0;
/// A start position this far from the last one is a new round's.
const SPAWN_MOVED_IN: f32 = 200.0;

#[derive(Default)]
pub struct ThreatMap {
    /// Centroid of each team's players during the freeze, by team number.
    spawns: HashMap<i32, [f32; 3]>,
    /// Shortest ways out from each team's start: (graph generation, start,
    /// parent of every node toward that start).
    flows: HashMap<i32, (u64, [f32; 3], Vec<u32>)>,
}

impl ThreatMap {
    /// Record where a team stands while frozen.
    pub fn note_spawn(&mut self, team: i32, centroid: [f32; 3]) {
        self.spawns.insert(team, centroid);
    }

    /// Rebuild a team's flow when its start moved or the graph changed.
    pub fn refresh(&mut self, graph: &NavGraph) {
        if graph.is_empty() {
            return;
        }
        for (&team, &spawn) in &self.spawns {
            let stale = self.flows.get(&team).is_none_or(|(generation, at, _)| {
                *generation != graph.generation
                    || dist_xy(*at, spawn) > SPAWN_MOVED_IN
            });
            if stale {
                let parents = flow_from(graph, spawn);
                self.flows.insert(team, (graph.generation, spawn, parents));
            }
        }
    }

    /// The point an enemy of `team` would come through on its way to `feet`:
    /// a little way back along the shortest route from the enemy start.
    pub fn approach(&self, graph: &NavGraph, enemy_team: i32, feet: [f32; 3]) -> Option<[f32; 3]> {
        let (generation, _, parents) = self.flows.get(&enemy_team)?;
        if *generation != graph.generation {
            return None;
        }
        let mut at = nav::nearest_within(graph, feet, nav::SNAP_IN)? as usize;
        let mut walked = 0.0;
        let start = graph.nodes[at];
        while walked < APPROACH_IN {
            let next = parents[at];
            if next == u32::MAX || next as usize == at {
                break;
            }
            walked += dist_xy(graph.nodes[at], graph.nodes[next as usize]);
            at = next as usize;
        }
        (dist_xy(start, graph.nodes[at]) > 64.0).then_some(graph.nodes[at])
    }
}

impl std::fmt::Debug for ThreatMap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ThreatMap")
            .field("spawns", &self.spawns)
            .field("flows", &self.flows.len())
            .finish()
    }
}

/// Dijkstra from the nodes around `spawn`; each node's parent is its next
/// step toward the spawn.
fn flow_from(graph: &NavGraph, spawn: [f32; 3]) -> Vec<u32> {
    let n = graph.nodes.len();
    let mut dist = vec![f32::INFINITY; n];
    let mut parent = vec![u32::MAX; n];
    let mut heap = BinaryHeap::new();
    let mut seeds: Vec<(f32, usize)> = graph
        .nodes
        .iter()
        .enumerate()
        .map(|(i, p)| (dist_xy(*p, spawn), i))
        .filter(|(d, _)| *d < 400.0)
        .collect();
    if seeds.is_empty()
        && let Some(i) = nav::nearest_within(graph, spawn, 2000.0)
    {
        seeds.push((0.0, i as usize));
    }
    for (_, i) in seeds {
        dist[i] = 0.0;
        parent[i] = i as u32;
        heap.push(Reverse((0u32, i as u32)));
    }
    // Non-negative floats order like their bit patterns.
    while let Some(Reverse((d_bits, i))) = heap.pop() {
        let i = i as usize;
        let d = f32::from_bits(d_bits);
        if d > dist[i] {
            continue;
        }
        // The flow runs the way an enemy walks: from the spawn outward, so
        // a node's parent is the node an enemy reached it from. Edges are
        // followed backwards (j -> i walked as i <- j) where the graph has
        // them both ways; one-way drops still count in the walking direction.
        for edge in &graph.adj[i] {
            let j = edge.to as usize;
            let nd = d + edge.cost;
            if nd < dist[j] {
                dist[j] = nd;
                parent[j] = i as u32;
                heap.push(Reverse((nd.to_bits(), j as u32)));
            }
        }
    }
    parent
}

fn dist_xy(a: [f32; 3], b: [f32; 3]) -> f32 {
    (a[0] - b[0]).hypot(a[1] - b[1])
}
