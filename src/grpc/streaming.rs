use crossbeam_queue::ArrayQueue;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::proto::RtbDataPoint;

/// A handle to an active RTB gRPC stream.
///
/// Data points are pushed into a bounded lock-free queue by a background
/// tokio task.  The FFI layer polls the queue non-blocking.
pub struct RtbStreamHandle {
    queue: Arc<ArrayQueue<RtbDataPoint>>,
    cancel: CancellationToken,
    _task: JoinHandle<()>,
}

impl RtbStreamHandle {
    /// Spawn a background task that reads from the gRPC stream and pushes
    /// data points into the internal queue.
    ///
    /// `capacity` is the max number of data points buffered before old ones
    /// are dropped (ring-buffer style).
    pub fn start(
        rt: &Runtime,
        mut stream: tonic::Streaming<RtbDataPoint>,
        capacity: usize,
    ) -> Self {
        let queue = Arc::new(ArrayQueue::new(capacity));
        let cancel = CancellationToken::new();

        let q = queue.clone();
        let ct = cancel.clone();
        let count = Arc::new(AtomicU64::new(0));
        let cnt = count.clone();

        let task = rt.spawn(async move {
            loop {
                tokio::select! {
                    _ = ct.cancelled() => {
                        log::info!("RTB stream cancelled");
                        break;
                    }
                    result = stream.message() => {
                        match result {
                            Ok(Some(dp)) => {
                                let n = cnt.fetch_add(1, Ordering::Relaxed);
                                if n < 3 {
                                    log::info!(
                                        "RTB recv #{}: ts={} fps={} power={} temp={} gpu={}MHz/{}%",
                                        n, dp.timestamp_ms, dp.fps, dp.power_mw,
                                        dp.battery_temp_c, dp.gpu_freq_mhz, dp.gpu_loading_pct
                                    );
                                }

                                // If the queue is full, pop the oldest item first
                                // then re-push. This avoids losing the new sample.
                                match q.push(dp) {
                                    Ok(()) => {}
                                    Err(rejected) => {
                                        let _ = q.pop();
                                        let _ = q.push(rejected);
                                        log::trace!("RTB queue full, dropped oldest sample");
                                    }
                                }
                            }
                            Ok(None) => {
                                log::info!("RTB stream ended (server closed)");
                                break;
                            }
                            Err(e) => {
                                log::error!("RTB stream error: {e}");
                                break;
                            }
                        }
                    }
                }
            }
        });

        Self {
            queue,
            cancel,
            _task: task,
        }
    }

    /// Non-blocking poll: returns the next data point if available.
    pub fn poll(&self) -> Option<RtbDataPoint> {
        self.queue.pop()
    }

    /// Cancel the background stream task.
    pub fn cancel(self) {
        self.cancel.cancel();
        // The JoinHandle is dropped, which is fine – the task will
        // notice the cancellation token and exit.
    }
}
