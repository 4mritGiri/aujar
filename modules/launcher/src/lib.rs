#[derive(Debug, Clone)]
pub struct LauncherQuery {
    pub text: String,
}

impl LauncherQuery {
    pub fn new(text: impl Into<String>) -> Self {
        Self { text: text.into() }
    }
}
