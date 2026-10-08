use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppInfo { pub name: String, pub version: String }

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Request {
    Status,
    Color { value: String },
    Rename { from: String, to: String, files: Vec<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Response { Ok { message: String }, Error { message: String } }

pub fn app_info() -> AppInfo { AppInfo { name: "Linux PowerToys".into(), version: env!("CARGO_PKG_VERSION").into() } }
