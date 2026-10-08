#[derive(Debug, Clone)]
pub struct ClipboardEntry {
    pub id: u64,
    pub text: String,
    pub pinned: bool,
}

#[derive(Default)]
pub struct ClipboardStore {
    entries: Vec<ClipboardEntry>,
}

impl ClipboardStore {
    pub fn push(&mut self, text: impl Into<String>) {
        let id = self.entries.len() as u64 + 1;
        self.entries.push(ClipboardEntry {
            id,
            text: text.into(),
            pinned: false,
        });
    }

    pub fn entries(&self) -> &[ClipboardEntry] {
        &self.entries
    }
}
