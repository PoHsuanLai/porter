//! How far under its origin a relay may go: the paths of an endpoint's server that a relay
//! planned for one grant may send requests to. The service decides this once, when it plans the
//! relay (porter-service's `reach`); a relay only applies the value it is handed.

/// What a path of a [`Reach`] lets through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ReachMethods {
    /// Every method.
    Any,
    /// `PROPFIND` with `Depth: 0`: one resource's properties, no listing.
    ReadProperties,
}

/// One path prefix a relay may reach, percent escapes decoded and without a trailing `/`; an
/// empty prefix is the whole origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathReach {
    prefix: Vec<u8>,
    methods: ReachMethods,
}

impl PathReach {
    /// `methods` under `prefix` (decoded bytes, no trailing `/`).
    pub fn new(prefix: Vec<u8>, methods: ReachMethods) -> Self {
        Self { prefix, methods }
    }

    /// The decoded path prefix.
    pub fn prefix(&self) -> &[u8] {
        &self.prefix
    }

    /// What it lets through.
    pub fn methods(&self) -> ReachMethods {
        self.methods
    }
}

/// The paths a relay may reach on its endpoint's origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reach {
    paths: Vec<PathReach>,
}

impl Reach {
    /// The whole origin, with every method.
    pub fn origin() -> Self {
        Self::under(vec![PathReach::new(Vec::new(), ReachMethods::Any)])
    }

    /// Only these paths.
    pub fn under(paths: Vec<PathReach>) -> Self {
        Self { paths }
    }

    /// Whether the whole origin is reachable, with every method.
    pub fn is_origin(&self) -> bool {
        self.paths
            .iter()
            .any(|p| p.prefix.is_empty() && p.methods == ReachMethods::Any)
    }

    /// The reachable paths.
    pub fn paths(&self) -> &[PathReach] {
        &self.paths
    }
}
