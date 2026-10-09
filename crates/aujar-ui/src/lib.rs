//! UI contracts live here so modules do not depend directly on a specific GUI toolkit.
use aujar_core::Capability;

#[derive(Debug, Clone)]
pub struct ModuleDescriptor { pub id: &'static str, pub name: &'static str, pub capabilities: &'static [Capability] }

pub trait UiModule { fn descriptor(&self) -> ModuleDescriptor; }
