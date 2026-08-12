use std::time::Duration;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio_stream::StreamExt;
use uuid::Uuid;

/// One decoded `event: sync` frame (`{entity, action, id}` — notify only).
#[derive(Debug, Clone)]
pub struct SyncFrame {
    pub entity: String,
    pub action: String,
    pub id: String,
}

/// A live subscription to the sync stream for one user/token. Dropping it
/// aborts the reader task, which drops the HTTP response → the server's
/// ConnGuard unregisters the connection.
pub struct SyncProbe {
    connection_id: Uuid,
    rx: mpsc::UnboundedReceiver<SyncFrame>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for SyncProbe {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl SyncProbe {
    /// Open the stream for `token`. Resolves once the `connected` handshake
    /// frame arrives (so `connection_id()` is immediately usable).
    ///
    /// Generic over the app's `TestServer` shim via the [`crate::ApiUrlTarget`]
    /// seam — the probe only needs the server's `/sync/subscribe` URL, never any
    /// app-side type. (Was `&crate::common::TestServer` pre-move.)
    pub async fn open<S: crate::ApiUrlTarget>(server: &S, token: &str) -> SyncProbe {
        let resp = reqwest::Client::new()
            .get(server.api_url("/sync/subscribe"))
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
            .expect("sync subscribe request failed");
        assert_eq!(
            resp.status(),
            200,
            "sync subscribe should return 200 for an authenticated user"
        );

        let (id_tx, id_rx) = oneshot::channel::<Uuid>();
        let (frame_tx, frame_rx) = mpsc::unbounded_channel::<SyncFrame>();

        let task = tokio::spawn(async move {
            let mut stream = resp.bytes_stream();
            let mut buf = String::new();
            let mut id_tx = Some(id_tx);
            while let Some(Ok(chunk)) = stream.next().await {
                buf.push_str(&String::from_utf8_lossy(&chunk));
                // SSE frames are separated by a blank line.
                while let Some(pos) = buf.find("\n\n") {
                    let frame: String = buf.drain(..pos + 2).collect();
                    let (event, data) = parse_sse_frame(&frame);
                    match event.as_deref() {
                        Some("connected") => {
                            if let Some(tx) = id_tx.take() {
                                if let Some(id) = data
                                    .as_deref()
                                    .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
                                    .and_then(|v| {
                                        v.get("connection_id")
                                            .and_then(|c| c.as_str())
                                            .and_then(|s| Uuid::parse_str(s).ok())
                                    })
                                {
                                    let _ = tx.send(id);
                                }
                            }
                        }
                        Some("sync") => {
                            if let Some(f) = data
                                .as_deref()
                                .and_then(|d| serde_json::from_str::<serde_json::Value>(d).ok())
                                .map(|v| SyncFrame {
                                    entity: v["entity"].as_str().unwrap_or_default().to_string(),
                                    action: v["action"].as_str().unwrap_or_default().to_string(),
                                    id: v["id"].as_str().unwrap_or_default().to_string(),
                                })
                            {
                                if frame_tx.send(f).is_err() {
                                    return; // receiver gone
                                }
                            }
                        }
                        _ => {} // keep-alive comments / unknown events
                    }
                }
            }
        });

        let connection_id = tokio::time::timeout(Duration::from_secs(5), id_rx)
            .await
            .expect("timed out waiting for the `connected` handshake frame")
            .expect("sync probe task ended before the handshake");

        SyncProbe {
            connection_id,
            rx: frame_rx,
            task,
        }
    }

    /// The server-assigned connection id (echo it back via the
    /// `X-Sync-Connection-Id` header to test self-echo suppression).
    pub fn connection_id(&self) -> Uuid {
        self.connection_id
    }

    /// Wait up to `timeout` for a `sync` frame matching `(entity, action)`,
    /// ignoring any other frames that arrive first (e.g. a dual-audience
    /// mutation also emits a second entity). Panics on timeout / stream close.
    pub async fn expect_event(
        &mut self,
        entity: &str,
        action: &str,
        timeout: Duration,
    ) -> SyncFrame {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(remaining, self.rx.recv()).await {
                Ok(Some(f)) if f.entity == entity && f.action == action => return f,
                Ok(Some(_)) => {} // a different event — keep waiting
                Ok(None) => {
                    panic!("sync stream closed while waiting for {entity}/{action}")
                }
                Err(_) => panic!("timed out waiting for sync event {entity}/{action}"),
            }
        }
    }

    /// Assert NO sync frame at all arrives within `dur` (cross-user isolation
    /// / origin-skip). A closed stream also counts as silence.
    pub async fn expect_silence(&mut self, dur: Duration) {
        match tokio::time::timeout(dur, self.rx.recv()).await {
            Ok(Some(f)) => panic!(
                "expected silence but received sync {}/{} (id {})",
                f.entity, f.action, f.id
            ),
            Ok(None) | Err(_) => {}
        }
    }

    /// Assert the server CLOSES the stream within `dur` (e.g. the periodic
    /// re-check tears it down after the account is deactivated or loses the
    /// baseline permission). Intervening data frames are ignored; a closed
    /// channel (`recv` → None) is the success condition.
    pub async fn expect_closed(&mut self, dur: Duration) {
        let deadline = tokio::time::Instant::now() + dur;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                panic!("expected the sync stream to close within {dur:?}, but it stayed open");
            }
            match tokio::time::timeout(remaining, self.rx.recv()).await {
                Ok(None) => return,      // server closed the stream — success
                Ok(Some(_)) => continue, // ignore data frames, keep waiting for close
                Err(_) => {
                    panic!("expected the sync stream to close within {dur:?}, but it stayed open")
                }
            }
        }
    }

    /// [`expect_event`](Self::expect_event) narrowed to a specific row id — needed
    /// whenever unrelated activity emits the same `(entity, action)` on the same
    /// stream (a second job running in the background, say). Without it such a test
    /// can match a bystander's frame and pass while the frame it cares about was
    /// never sent.
    pub async fn expect_event_for(
        &mut self,
        entity: &str,
        action: &str,
        id: &str,
        timeout: Duration,
    ) -> SyncFrame {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(remaining, self.rx.recv()).await {
                Ok(Some(f)) if f.entity == entity && f.action == action && f.id == id => return f,
                Ok(Some(_)) => {} // a different row / event — keep waiting
                Ok(None) => {
                    panic!("sync stream closed while waiting for {entity}/{action} id={id}")
                }
                Err(_) => panic!("timed out waiting for sync event {entity}/{action} id={id}"),
            }
        }
    }

    /// Like `expect_event`, but matches the FIRST frame whose entity is in
    /// `entities` (and whose action matches) — for a dual-audience mutation
    /// that emits two distinct entities in an unspecified order, so a fixed
    /// single-entity `expect_event` could drop the sibling frame.
    pub async fn expect_event_any(
        &mut self,
        entities: &[&str],
        action: &str,
        timeout: Duration,
    ) -> SyncFrame {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            match tokio::time::timeout(remaining, self.rx.recv()).await {
                Ok(Some(f)) if entities.contains(&f.entity.as_str()) && f.action == action => {
                    return f;
                }
                Ok(Some(_)) => {}
                Ok(None) => {
                    panic!("sync stream closed while waiting for {entities:?}/{action}")
                }
                Err(_) => panic!("timed out waiting for sync event {entities:?}/{action}"),
            }
        }
    }
}

/// Pull `event:` + concatenated `data:` lines out of one raw SSE frame.
fn parse_sse_frame(frame: &str) -> (Option<String>, Option<String>) {
    let mut event = None;
    let mut data_lines: Vec<String> = Vec::new();
    for line in frame.lines() {
        if let Some(rest) = line.strip_prefix("event:") {
            event = Some(rest.trim().to_string());
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.strip_prefix(' ').unwrap_or(rest).to_string());
        }
        // ':' keep-alive comments and blank lines are ignored.
    }
    let data = if data_lines.is_empty() {
        None
    } else {
        Some(data_lines.join("\n"))
    };
    (event, data)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Frame decoding is the probe's ONLY pure logic, and everything the probe can
    // assert rests on it: a decoder that silently returned `None` would make
    // `expect_silence` pass unconditionally — i.e. the failure mode is a test tool
    // that reports success no matter what the server did. These came from
    // CytoAnalyst's hand-rolled mirror of this file, which had them while this
    // upstream copy did not; they travel with the code they cover.

    #[test]
    fn decodes_a_sync_frame() {
        let raw = "event: sync\ndata: {\"entity\":\"dataset\",\"action\":\"update\",\"id\":\"00000000-0000-0000-0000-000000000001\"}\n\n";
        let (event, data) = parse_sse_frame(raw);
        assert_eq!(event.as_deref(), Some("sync"));
        let v: serde_json::Value = serde_json::from_str(&data.expect("data")).expect("json");
        assert_eq!(v["entity"], "dataset");
        assert_eq!(v["action"], "update");
        assert_eq!(v["id"], "00000000-0000-0000-0000-000000000001");
    }

    #[test]
    fn a_connected_handshake_is_named_connected_not_sync() {
        let raw = "event: connected\ndata: {\"connection_id\":\"00000000-0000-0000-0000-0000000000ff\"}\n\n";
        let (event, data) = parse_sse_frame(raw);
        assert_eq!(
            event.as_deref(),
            Some("connected"),
            "the handshake must not be mistaken for a data frame"
        );
        let v: serde_json::Value = serde_json::from_str(&data.expect("data")).expect("json");
        assert_eq!(v["connection_id"], "00000000-0000-0000-0000-0000000000ff");
    }

    #[test]
    fn keepalive_comments_decode_to_nothing() {
        assert_eq!(parse_sse_frame(":\n\n"), (None, None));
    }

    #[test]
    fn multi_line_data_is_rejoined_with_newlines() {
        // Per the SSE spec successive `data:` lines are one payload joined by "\n".
        let (_, data) = parse_sse_frame("event: sync\ndata: {\"a\":1,\ndata: \"b\":2}\n\n");
        assert_eq!(data.as_deref(), Some("{\"a\":1,\n\"b\":2}"));
    }
}
