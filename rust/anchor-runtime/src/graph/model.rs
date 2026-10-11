use super::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphSnapshot {
    pub objective: String,
    #[serde(default)]
    pub input: Value,
    pub entry: String,
    #[serde(default)]
    pub agents: BTreeMap<String, AgentDefinition>,
    #[serde(default)]
    pub ops: BTreeMap<String, Value>,
    pub nodes: Vec<GraphNode>,
    #[serde(default)]
    pub edges: Vec<GraphEdge>,
    #[serde(default, rename = "_module_rounds")]
    pub module_rounds: BTreeMap<String, u32>,
}

/// A statically validated, non-nested region activated by one fanout node.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParallelRegion {
    pub fanout: String,
    pub join: String,
    /// Ordered linear paths, excluding the fanout and join control nodes.
    pub branches: Vec<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentDefinition {
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub instructions: String,
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub max_steps: Option<u64>,
    #[serde(default)]
    pub wall_time_limit_seconds: Option<f64>,
    #[serde(default)]
    pub reads: Vec<String>,
    #[serde(default)]
    pub writes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphNode {
    pub id: String,
    #[serde(default)]
    pub agent: Option<String>,
    #[serde(default)]
    pub op: Option<String>,
    #[serde(default, rename = "with")]
    pub input: Option<Value>,
    #[serde(default)]
    pub plugins: Vec<PluginRef>,
    #[serde(default)]
    pub max_rounds: Option<u32>,
}

/// One Plugin a node declares, and whether the Graph can run without it.
///
/// The Graph says what it needs; a deployment or a session may switch an
/// *optional* Plugin off, but never a required one. A bare string in an older
/// Graph means `required: true`, which is what "declared" always meant.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "PluginRefRepr")]
pub struct PluginRef {
    pub id: String,
    pub required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default: Option<bool>,
}

/// A reference as it is *written*: a bare id, or the full object.
///
/// Snapshots and Run records written before the object form carry `["id"]`; they
/// must keep loading, and a bare id has always meant "required".
#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
enum PluginRefRepr {
    Id(String),
    Full {
        id: String,
        #[serde(default = "default_true")]
        required: bool,
        #[serde(default)]
        default: Option<bool>,
    },
}

/// A `PluginRef` whose meaning is exactly the old bare id **writes** the bare id
/// back.
///
/// `GraphSnapshot::digest()` hashes the snapshot's serialization, so a record
/// written before the object form existed can only reproduce its own digest if a
/// declaration that says nothing beyond "required" serializes the way it was
/// written. `required: true` with no `default` is precisely the old form --
/// whether it arrived as a string or as `{"id":…,"required":true}` -- so both
/// collapse to `"id"` and old records keep validating. An optional reference, or
/// one that names a default, still writes the object.
impl Serialize for PluginRef {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        if self.required && self.default.is_none() {
            return serializer.serialize_str(&self.id);
        }
        use serde::ser::SerializeStruct;
        let mut state = serializer.serialize_struct("PluginRef", 3)?;
        state.serialize_field("id", &self.id)?;
        state.serialize_field("required", &self.required)?;
        if let Some(default) = self.default {
            state.serialize_field("default", &default)?;
        }
        state.end()
    }
}

impl From<PluginRefRepr> for PluginRef {
    fn from(repr: PluginRefRepr) -> Self {
        match repr {
            PluginRefRepr::Id(id) => Self::required(id),
            PluginRefRepr::Full {
                id,
                required,
                default,
            } => Self {
                id,
                required,
                default,
            },
        }
    }
}

fn default_true() -> bool {
    true
}

impl PluginRef {
    pub fn required(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            required: true,
            default: None,
        }
    }
}

impl From<&str> for PluginRef {
    fn from(id: &str) -> Self {
        Self::required(id)
    }
}

impl From<String> for PluginRef {
    fn from(id: String) -> Self {
        Self::required(id)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NodeKind {
    Agent,
    OpRun,
    OpHost,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GraphEdge {
    #[serde(rename = "from")]
    pub from_node: String,
    #[serde(rename = "to")]
    pub to_node: String,
}
