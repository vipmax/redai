use notify::{RecursiveMode, Watcher, recommended_watcher};
use std::collections::HashSet;
use std::path::PathBuf;
use tokio::sync::mpsc;
use tokio::sync::mpsc::Receiver;

pub struct FsWatcher {
    watcher: notify::RecommendedWatcher,
    pub watch_rx: Receiver<notify::Result<notify::Event>>,
    watched_paths: HashSet<PathBuf>,
}

impl FsWatcher {
    pub fn new() -> Self {
        let (watch_tx, watch_rx) = mpsc::channel(5);

        let watcher = recommended_watcher(move |res| {
            let _ = watch_tx.blocking_send(res);
        })
        .expect("Failed to create watcher");

        Self {
            watcher,
            watch_rx,
            watched_paths: HashSet::new(),
        }
    }

    pub(crate) fn sync<I>(&mut self, paths: I) -> anyhow::Result<()>
    where
        I: IntoIterator<Item = PathBuf>,
    {
        let desired = paths.into_iter().collect::<HashSet<_>>();

        let to_unwatch = self
            .watched_paths
            .difference(&desired)
            .cloned()
            .collect::<Vec<_>>();

        let to_watch = desired
            .difference(&self.watched_paths)
            .cloned()
            .collect::<Vec<_>>();

        for path in to_unwatch {
            self.watcher.unwatch(&path)?;
        }

        for path in &to_watch {
            self.watcher.watch(path, RecursiveMode::NonRecursive)?;
        }

        self.watched_paths = desired;
        Ok(())
    }
}
