//! One long-lived follower per host: acc's fan-in script tails every
//! attached session's log and exit file on a single channel. Changing the
//! set of files restarts it from the offsets already consumed.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use acc_core::detached::{FanInParser, FeedEntry, fan_in_script};
use chameleon_app::{AppEvent, HostId};
use tokio::io::AsyncReadExt as _;
use tokio::process::Child;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use super::Host;

const COALESCE: Duration = Duration::from_millis(250);

#[derive(Debug, Default)]
struct Inner {
    entries: HashMap<String, FeedEntry>,
    child: Option<Child>,
    reader: Option<JoinHandle<()>>,
    restart: Option<JoinHandle<()>>,
    generation: u64,
    failures: u32,
    closed: bool,
}

#[derive(Debug, Clone)]
pub struct Feed {
    host: Arc<dyn Host>,
    id: HostId,
    tx: UnboundedSender<AppEvent>,
    inner: Arc<Mutex<Inner>>,
}

fn lock(m: &Mutex<Inner>) -> MutexGuard<'_, Inner> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Feed {
    pub fn new(host: Arc<dyn Host>, tx: UnboundedSender<AppEvent>) -> Self {
        Self {
            id: host.name().to_owned(),
            host,
            tx,
            inner: Arc::new(Mutex::new(Inner::default())),
        }
    }

    pub fn follow(&self, entry: FeedEntry) {
        let mut g = lock(&self.inner);
        if g.entries.contains_key(&entry.key) {
            return;
        }
        g.entries.insert(entry.key.clone(), entry);
        drop(g);
        self.schedule(COALESCE);
    }

    pub fn unfollow(&self, key: &str) {
        let mut g = lock(&self.inner);
        if g.entries.remove(key).is_none() {
            return;
        }
        drop(g);
        self.schedule(COALESCE);
    }

    fn schedule(&self, delay: Duration) {
        let mut g = lock(&self.inner);
        if g.closed {
            return;
        }
        if let Some(t) = g.restart.take() {
            t.abort();
        }
        let me = self.clone();
        g.restart = Some(tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            me.restart().await;
        }));
    }

    async fn restart(&self) {
        let (old_child, entries, generation) = {
            let mut g = lock(&self.inner);
            if let Some(r) = g.reader.take() {
                r.abort();
            }
            g.generation += 1;
            (
                g.child.take(),
                g.entries.values().cloned().collect::<Vec<_>>(),
                g.generation,
            )
        };
        if let Some(mut c) = old_child {
            super::stop(self.host.as_ref(), &mut c).await;
        }
        if entries.is_empty() || lock(&self.inner).closed {
            return;
        }
        let script = fan_in_script(&entries);
        let mut child = match super::spawn_long(self.host.as_ref(), &script) {
            Ok(c) => c,
            Err(e) => {
                self.failed(e);
                return;
            }
        };
        let Some(mut stdout) = child.stdout.take() else {
            self.failed("no stdout".into());
            return;
        };
        let parser = FanInParser::new(&entries);
        let me = self.clone();
        let reader = tokio::spawn(async move {
            let mut parser = parser;
            let mut buf = vec![0u8; 64 * 1024];
            let mut announced = false;
            loop {
                let n = match stdout.read(&mut buf).await {
                    Ok(0) | Err(_) => break,
                    Ok(n) => n,
                };
                let lines = parser.feed(&buf[..n]);
                if lines.is_empty() {
                    continue;
                }
                {
                    let mut g = lock(&me.inner);
                    if g.generation != generation {
                        return;
                    }
                    g.failures = 0;
                    for l in &lines {
                        if let Some(e) = g.entries.get_mut(&l.key) {
                            e.offset = l.offset;
                        }
                    }
                }
                if !announced {
                    announced = true;
                    let _ = me.tx.send(AppEvent::FeedUp { host: me.id.clone() });
                }
                for l in lines {
                    let _ = me.tx.send(AppEvent::FeedLine {
                        host: me.id.clone(),
                        key: l.key,
                        bytes: l.bytes,
                    });
                }
            }
            // Ended without being asked to: retry with backoff.
            let current = lock(&me.inner).generation == generation;
            if current {
                me.failed("the follower ended".into());
            }
        });
        let mut g = lock(&self.inner);
        g.child = Some(child);
        g.reader = Some(reader);
    }

    fn failed(&self, error: String) {
        let delay = {
            let mut g = lock(&self.inner);
            if g.closed {
                return;
            }
            g.failures += 1;
            let n = g.failures.min(6);
            Duration::from_millis(500 * (1u64 << n))
        };
        let _ = self.tx.send(AppEvent::FeedDown {
            host: self.id.clone(),
            error,
        });
        self.schedule(delay.min(Duration::from_secs(30)));
    }

    /// Stops following; the harnesses keep running on the host.
    pub async fn close(&self) {
        let child = {
            let mut g = lock(&self.inner);
            g.closed = true;
            if let Some(t) = g.restart.take() {
                t.abort();
            }
            if let Some(r) = g.reader.take() {
                r.abort();
            }
            g.child.take()
        };
        if let Some(mut c) = child {
            super::stop(self.host.as_ref(), &mut c).await;
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use std::io::Write as _;

    use acc_core::detached::FeedPath;
    use tokio::sync::mpsc::UnboundedReceiver;

    use super::*;

    async fn next_line(rx: &mut UnboundedReceiver<AppEvent>) -> (String, String) {
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(10), rx.recv())
                .await
                .expect("a line in time")
                .expect("channel open");
            if let AppEvent::FeedLine { key, bytes, .. } = ev {
                return (key, String::from_utf8(bytes).unwrap());
            }
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn follows_files_and_resumes_after_a_restart() {
        let dir = std::env::temp_dir().join(format!("cham-feed-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let a = dir.join("a.log");
        let b = dir.join("b.log");
        std::fs::write(&a, "one\n").unwrap();
        std::fs::write(&b, "").unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let host: Arc<dyn Host> = Arc::new(crate::host::local::LocalHost::new("local"));
        let feed = Feed::new(host, tx);
        feed.follow(FeedEntry {
            key: "a".into(),
            path: FeedPath::File(a.display().to_string()),
            offset: 0,
        });
        assert_eq!(next_line(&mut rx).await, ("a".into(), "one\n".into()));
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&a).unwrap();
            f.write_all(b"two\n").unwrap();
        }
        assert_eq!(next_line(&mut rx).await, ("a".into(), "two\n".into()));
        assert_eq!(lock(&feed.inner).entries["a"].offset, 8);

        // Adding a file restarts the follower; `a` resumes after what was read.
        feed.follow(FeedEntry {
            key: "b".into(),
            path: FeedPath::File(b.display().to_string()),
            offset: 0,
        });
        tokio::time::sleep(Duration::from_millis(600)).await;
        {
            let mut f = std::fs::OpenOptions::new().append(true).open(&b).unwrap();
            f.write_all(b"bee\n").unwrap();
            let mut f = std::fs::OpenOptions::new().append(true).open(&a).unwrap();
            f.write_all(b"three\n").unwrap();
        }
        let mut got = vec![next_line(&mut rx).await, next_line(&mut rx).await];
        got.sort();
        assert_eq!(
            got,
            vec![("a".into(), "three\n".into()), ("b".into(), "bee\n".into())],
            "nothing repeated, nothing lost"
        );
        feed.close().await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        // The follower's processes are gone with it.
        let out = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("pgrep -f '^tail -c .* -F {}' || true", a.display()))
            .output()
            .unwrap();
        assert!(
            out.stdout.is_empty(),
            "tails still running: {}",
            String::from_utf8_lossy(&out.stdout)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
