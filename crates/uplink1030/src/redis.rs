//! Acknowledged per-event Redis PUBLISH, matching datalink's immediate-output policy.
//! No detached output task, timer-dependent batch or unbounded queue. At most one
//! event is unacknowledged; uncertain retries may duplicate it (never exactly-once).
use ::redis::{aio::MultiplexedConnection, Client};
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};
use tokio::time::{sleep, timeout};

pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const FINISH_TIMEOUT: Duration = Duration::from_secs(2);

pub struct Publisher {
    client: Client,
    connection: Option<MultiplexedConnection>,
    topic: String,
    retry: Duration,
    pending: Option<String>,
}
impl Publisher {
    pub async fn connect(url: &str, topic: String, retry: Duration) -> Result<Self, String> {
        if topic.is_empty() {
            return Err("Redis topic must not be empty".into());
        }
        let client = Client::open(url).map_err(|e| format!("Invalid Redis URL: {e}"))?;
        let connection = timeout(CONNECT_TIMEOUT, client.get_multiplexed_async_connection())
            .await
            .map_err(|_| "Redis initial connection timed out".to_owned())?
            .map_err(|e| format!("Redis initial connection failed: {e}"))?;
        Ok(Self {
            client,
            connection: Some(connection),
            topic,
            retry,
            pending: None,
        })
    }

    async fn send_once(&mut self) -> Result<(), (String, bool)> {
        // A cancelled or failed query must not reuse a potentially desynchronised
        // connection. It may already have executed, even without an acknowledgement.
        let mut connection = match self.connection.take() {
            Some(c) => c,
            None => timeout(
                CONNECT_TIMEOUT,
                self.client.get_multiplexed_async_connection(),
            )
            .await
            .map_err(|_| ("Redis reconnect timed out".into(), false))?
            .map_err(|e| (e.to_string(), false))?,
        };
        let payload = self.pending.as_ref().expect("pending Redis event");
        // A zero subscriber count is a successful PUBLISH acknowledgement, not an error.
        let _: i64 = ::redis::cmd("PUBLISH")
            .arg(&self.topic)
            .arg(payload)
            .query_async(&mut connection)
            .await
            .map_err(|e| (e.to_string(), true))?;
        self.connection = Some(connection);
        self.pending = None;
        Ok(())
    }

    /// Returns false on interruption or when retries are disabled and a publish fails.
    /// Console/file output may still retain the decoded event in those cases.
    pub async fn publish(&mut self, payload: &str, stopped: &AtomicBool) -> bool {
        if stopped.load(Ordering::Relaxed) {
            return false;
        }
        assert!(
            self.pending.is_none(),
            "previous Redis event must be resolved before accepting another"
        );
        self.pending = Some(payload.to_owned());
        let stop = cancelled(stopped);
        tokio::pin!(stop);
        loop {
            let result = tokio::select! {
                biased;
                _ = &mut stop => {
                    eprintln!("uplink1030: Redis publish interrupted; final drain may repeat an already-delivered event");
                    return false;
                },
                result = self.send_once() => result,
            };
            match result {
                Ok(()) => return true,
                Err((error, uncertain)) if self.retry.is_zero() => {
                    eprintln!("uplink1030: Redis publish failed: {error}; retries disabled; skipping one unacknowledged event (delivery uncertain={uncertain})");
                    self.pending = None;
                    return false;
                }
                Err((error, uncertain)) => {
                    eprintln!("uplink1030: Redis publish failed: {error}; reconnecting in {}s (delivery uncertain={uncertain}; retry may duplicate)", self.retry.as_secs_f64());
                    tokio::select! {
                        biased;
                        _ = &mut stop => return false,
                        _ = sleep(self.retry) => {},
                    }
                }
            }
        }
    }

    /// Explicitly await the one possible in-flight event at shutdown, even after
    /// Ctrl-C. Bound the whole drain, including reconnects and retries, to two seconds.
    pub async fn finish(&mut self) -> Result<(), String> {
        if self.pending.is_none() {
            return Ok(());
        }
        eprintln!(
            "uplink1030: draining one unacknowledged Redis event; a previous delivery may repeat"
        );
        timeout(FINISH_TIMEOUT, async {
            loop {
                match self.send_once().await {
                    Ok(()) => return Ok(()),
                    Err((error, _)) if self.retry.is_zero() => return Err(format!("Redis shutdown delivery failed: {error}; one event remains unacknowledged and may have been delivered")),
                    Err(_) => sleep(self.retry).await,
                }
            }
        }).await.map_err(|_| "Timed out draining Redis after 2s; one event remains unacknowledged and may have been delivered".to_owned())?
    }
}

pub async fn cancelled(stopped: &AtomicBool) {
    while !stopped.load(Ordering::Relaxed) {
        sleep(Duration::from_millis(20)).await;
    }
}
