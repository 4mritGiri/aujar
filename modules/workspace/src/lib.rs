#[derive(Debug, Clone, PartialEq, Eq)] pub struct Workspace { pub index:u32, pub name:String }
pub fn defaults(count:u32)->Vec<Workspace>{(1..=count).map(|i|Workspace{index:i,name:format!("Workspace {i}")}).collect()}
#[cfg(test)] mod tests { use super::*; #[test] fn defaults_have_names(){assert_eq!(defaults(2)[1].name,"Workspace 2");} }
