//! Projection layer: `Projector` down-projects the rich graph into the
//! existing UA `knowledge-graph.json` (nodes/edges + layers + tour).
//!
//! This layer is built in stages. The first concrete pieces classify graph
//! nodes into architectural layers and generate a guided tour over the graph.

#[path = "projection/layers.rs"]
pub mod layers;

#[path = "projection/tour.rs"]
pub mod tour;

#[path = "projection/projector.rs"]
pub mod projector;

pub use layers::{build_layers, classify_path, Layer, LayerKind};
pub use projector::{ProjectionStats, Projector, UaEdge, UaGraph, UaNode, UaProject};
pub use tour::{generate_tour, TourStep};
