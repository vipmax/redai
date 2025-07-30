use notify::{recommended_watcher, Watcher, RecursiveMode};
use std::path::Path;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;

pub struct FsWatcher {
    watcher: notify::RecommendedWatcher,
    pub watch_rx: Receiver<notify::Result<notify::Event>>,
}

impl FsWatcher {
    pub fn new() -> Self {
        let (watch_tx, watch_rx) = mpsc::channel(5);

        let watcher = recommended_watcher(move |res| {
            let _ = watch_tx.blocking_send(res);
        }).unwrap();

        Self { watcher, watch_rx }
    }

    pub(crate) fn add(&mut self, path: &Path) -> anyhow::Result<()> {
        self.watcher.watch(path, RecursiveMode::NonRecursive)?;
        Ok(())
    }
}
