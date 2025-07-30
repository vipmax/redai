use std::time::{Duration, Instant};
use crate::diff::diff_without_unchanged;

pub struct Snapshot {
    pub timestamp: Instant,
    pub content: String,
}

pub struct Tracker {
    snapshots: Vec<Snapshot>,
    max_age: Duration,
    max_versions: usize,
}

impl Tracker {
    pub fn new(initial: String) -> Self {
        Self {
            snapshots: vec![Snapshot {
                timestamp: Instant::now(),
                content: initial,
            }],
            max_age: Duration::from_secs(60 * 30),
            max_versions: 50,
        }
    }

    pub fn update(&mut self, new_content: String) {
        if self.latest() == new_content {
            return;
        }

        self.snapshots.push(Snapshot {
            timestamp: Instant::now(),
            content: new_content,
        });

        self.cleanup();
    }

    fn cleanup(&mut self) {
        let now = Instant::now();
        self.snapshots
            .retain(|s| now.duration_since(s.timestamp) <= self.max_age);

        if self.snapshots.len() > self.max_versions {
            let excess = self.snapshots.len() - self.max_versions;
            self.snapshots.drain(0..excess);
        }
    }

    fn latest(&self) -> String {
        self.snapshots
            .last()
            .map(|s| s.content.clone())
            .unwrap_or_default()
    }

    fn oldest(&self) -> Option<String> {
        self.snapshots.first().map(|s| s.content.clone())
    }

    pub fn summarize_recent_edits(&self) -> String {
        let maybe_oldest = self.oldest();
        let maybe_last = self.snapshots.last();
        match (maybe_oldest, maybe_last) {
            (Some(prev), Some(latest)) => {
                diff_without_unchanged(&prev, &latest.content)
            }
            _ => String::new(),
        }
    }
    
    pub fn snapshots(&self) -> &Vec<Snapshot> {
        &self.snapshots
    }
}