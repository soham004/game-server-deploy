//! Server instances: their definition on disk, the registry of all instances, the launch
//! command they run with, and the lifecycle service.

pub mod definition;
pub mod launch;
pub mod layout;
pub mod registry;
mod service;
pub mod settings;

pub use definition::{InstanceDefinition, Ports, SCHEMA_VERSION, SandboxIds};
pub use layout::InstanceLayout;
pub use registry::{AllocationPolicy, Instance, InstanceRegistry, NewInstance};
pub use service::{InstanceError, InstanceService};
pub use settings::{SettingsChange, SettingsError};
