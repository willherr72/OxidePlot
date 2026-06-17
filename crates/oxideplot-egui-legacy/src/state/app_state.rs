use serde::{Deserialize, Serialize};
use crate::state::graph_state::GraphState;
use crate::state::theme::Theme;

pub const VERSION: &str = "0.1.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppState {
    pub graphs: Vec<GraphState>,
    pub theme: Theme,
}

impl AppState {
    pub fn new() -> Self {
        let mut state = Self {
            graphs: Vec::new(),
            theme: Theme::default(),
        };
        // Start with one empty graph
        state.graphs.push(GraphState::new());
        state
    }

    pub fn add_graph(&mut self) -> &mut GraphState {
        self.graphs.push(GraphState::new());
        self.graphs.last_mut().unwrap()
    }

    pub fn remove_graph(&mut self, graph_id: u64) {
        // Remove sync references from other graphs
        let _partner_ids: Vec<u64> = self
            .graphs
            .iter()
            .find(|g| g.id == graph_id)
            .map(|g| g.sync_partner_ids.clone())
            .unwrap_or_default();

        for g in &mut self.graphs {
            g.sync_partner_ids.retain(|id| *id != graph_id);
        }

        self.graphs.retain(|g| g.id != graph_id);

        // Recompute sync groups after removal.
        self.recompute_sync_groups();
    }

    /// Recompute `sync_group_id` on every graph.
    ///
    /// The group id is the minimum graph-id among the graph itself and all of
    /// its sync partners.  Graphs that have no sync partners get `None`.
    pub fn recompute_sync_groups(&mut self) {
        for g in &mut self.graphs {
            if g.sync_partner_ids.is_empty() {
                g.sync_group_id = None;
            } else {
                let min_partner = g.sync_partner_ids.iter().copied().min().unwrap_or(g.id);
                g.sync_group_id = Some(g.id.min(min_partner));
            }
        }
    }

    pub fn graph_by_id(&self, id: u64) -> Option<&GraphState> {
        self.graphs.iter().find(|g| g.id == id)
    }

    pub fn graph_by_id_mut(&mut self, id: u64) -> Option<&mut GraphState> {
        self.graphs.iter_mut().find(|g| g.id == id)
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}
