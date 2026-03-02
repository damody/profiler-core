use crossbeam_queue::ArrayQueue;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::proto::RtbDataPoint;
use crate::proto::CrDataPoint;
use crate::proto::TcDataPoint;
use crate::proto::CmlDataPoint;
use crate::proto::GcDataPoint;

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
                                        dp.board_temp_c, dp.gpu_freq_mhz, dp.gpu_loading_pct
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

/// A handle to an active CR (Cache Report) gRPC stream.
///
/// Data points are pushed into a bounded lock-free queue by a background
/// tokio task.  The FFI layer polls the queue non-blocking.
pub struct CrStreamHandle {
    queue: Arc<ArrayQueue<CrDataPoint>>,
    cancel: CancellationToken,
    _task: JoinHandle<()>,
}

impl CrStreamHandle {
    /// Spawn a background task that reads from the gRPC stream and pushes
    /// data points into the internal queue.
    pub fn start(
        rt: &Runtime,
        mut stream: tonic::Streaming<CrDataPoint>,
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
                        log::info!("CR stream cancelled");
                        break;
                    }
                    result = stream.message() => {
                        match result {
                            Ok(Some(dp)) => {
                                let n = cnt.fetch_add(1, Ordering::Relaxed);
                                if n < 3 {
                                    log::info!(
                                        "CR recv #{}: ts={} cpus={}",
                                        n, dp.timestamp_ms, dp.cpus.len()
                                    );
                                }

                                match q.push(dp) {
                                    Ok(()) => {}
                                    Err(rejected) => {
                                        let _ = q.pop();
                                        let _ = q.push(rejected);
                                        log::trace!("CR queue full, dropped oldest sample");
                                    }
                                }
                            }
                            Ok(None) => {
                                log::info!("CR stream ended (server closed)");
                                break;
                            }
                            Err(e) => {
                                log::error!("CR stream error: {e}");
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
    pub fn poll(&self) -> Option<CrDataPoint> {
        self.queue.pop()
    }

    /// Cancel the background stream task.
    pub fn cancel(self) {
        self.cancel.cancel();
    }
}

/// A handle to an active TC (Thread Cache) gRPC stream.
///
/// Data points are pushed into a bounded lock-free queue by a background
/// tokio task.  The FFI layer polls the queue non-blocking.
pub struct TcStreamHandle {
    queue: Arc<ArrayQueue<TcDataPoint>>,
    cancel: CancellationToken,
    _task: JoinHandle<()>,
}

impl TcStreamHandle {
    /// Spawn a background task that reads from the gRPC stream and pushes
    /// data points into the internal queue.
    pub fn start(
        rt: &Runtime,
        mut stream: tonic::Streaming<TcDataPoint>,
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
                        log::info!("TC stream cancelled");
                        break;
                    }
                    result = stream.message() => {
                        match result {
                            Ok(Some(dp)) => {
                                let n = cnt.fetch_add(1, Ordering::Relaxed);
                                if n < 3 {
                                    log::info!(
                                        "TC recv #{}: ts={} threads={}",
                                        n, dp.timestamp_ms, dp.threads.len()
                                    );
                                }

                                match q.push(dp) {
                                    Ok(()) => {}
                                    Err(rejected) => {
                                        let _ = q.pop();
                                        let _ = q.push(rejected);
                                        log::trace!("TC queue full, dropped oldest sample");
                                    }
                                }
                            }
                            Ok(None) => {
                                log::info!("TC stream ended (server closed)");
                                break;
                            }
                            Err(e) => {
                                log::error!("TC stream error: {e}");
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
    pub fn poll(&self) -> Option<TcDataPoint> {
        self.queue.pop()
    }

    /// Cancel the background stream task.
    pub fn cancel(self) {
        self.cancel.cancel();
    }
}

/// A handle to an active CML (Cache/Memory Latency) gRPC stream.
///
/// CML is a finite-length benchmark (typically ~17 data points).
/// The background task sets `is_finished` when the stream ends naturally.
pub struct CmlStreamHandle {
    queue: Arc<ArrayQueue<CmlDataPoint>>,
    cancel: CancellationToken,
    _task: JoinHandle<()>,
    finished: Arc<std::sync::atomic::AtomicBool>,
}

impl CmlStreamHandle {
    /// Spawn a background task that reads from the gRPC stream and pushes
    /// data points into the internal queue.
    pub fn start(
        rt: &Runtime,
        mut stream: tonic::Streaming<CmlDataPoint>,
        capacity: usize,
    ) -> Self {
        let queue = Arc::new(ArrayQueue::new(capacity));
        let cancel = CancellationToken::new();
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));

        let q = queue.clone();
        let ct = cancel.clone();
        let fin = finished.clone();
        let count = Arc::new(AtomicU64::new(0));
        let cnt = count.clone();

        let task = rt.spawn(async move {
            loop {
                tokio::select! {
                    _ = ct.cancelled() => {
                        log::info!("CML stream cancelled");
                        break;
                    }
                    result = stream.message() => {
                        match result {
                            Ok(Some(dp)) => {
                                let n = cnt.fetch_add(1, Ordering::Relaxed);
                                if n < 3 {
                                    log::info!(
                                        "CML recv #{}: footprint={}KB cpus={}",
                                        n, dp.footprint_kb, dp.cpus.len()
                                    );
                                }

                                match q.push(dp) {
                                    Ok(()) => {}
                                    Err(rejected) => {
                                        let _ = q.pop();
                                        let _ = q.push(rejected);
                                        log::trace!("CML queue full, dropped oldest sample");
                                    }
                                }
                            }
                            Ok(None) => {
                                log::info!("CML stream ended (benchmark complete)");
                                fin.store(true, Ordering::Release);
                                break;
                            }
                            Err(e) => {
                                log::error!("CML stream error: {e}");
                                fin.store(true, Ordering::Release);
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
            finished,
        }
    }

    /// Non-blocking poll: returns the next data point if available.
    pub fn poll(&self) -> Option<CmlDataPoint> {
        self.queue.pop()
    }

    /// Returns true if the background stream task has finished
    /// (benchmark complete or error).
    pub fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    /// Cancel the background stream task.
    pub fn cancel(self) {
        self.cancel.cancel();
    }
}

/// A handle to an active GC (GPU Counters) gRPC stream.
///
/// Data points are pushed into a bounded lock-free queue by a background
/// tokio task.  The FFI layer polls the queue non-blocking.
pub struct GcStreamHandle {
    queue: Arc<ArrayQueue<GcDataPoint>>,
    cancel: CancellationToken,
    _task: JoinHandle<()>,
}

impl GcStreamHandle {
    /// Spawn a background task that reads from the gRPC stream and pushes
    /// data points into the internal queue.
    pub fn start(
        rt: &Runtime,
        mut stream: tonic::Streaming<GcDataPoint>,
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
                        log::info!("GC stream cancelled");
                        break;
                    }
                    result = stream.message() => {
                        match result {
                            Ok(Some(dp)) => {
                                let n = cnt.fetch_add(1, Ordering::Relaxed);
                                if n < 3 {
                                    log::info!(
                                        "GC recv #{}: ts={} counters={}",
                                        n, dp.timestamp_ms, dp.counters.len()
                                    );
                                }

                                match q.push(dp) {
                                    Ok(()) => {}
                                    Err(rejected) => {
                                        let _ = q.pop();
                                        let _ = q.push(rejected);
                                        log::trace!("GC queue full, dropped oldest sample");
                                    }
                                }
                            }
                            Ok(None) => {
                                log::info!("GC stream ended (server closed)");
                                break;
                            }
                            Err(e) => {
                                log::error!("GC stream error: {e}");
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
    pub fn poll(&self) -> Option<GcDataPoint> {
        self.queue.pop()
    }

    /// Cancel the background stream task.
    pub fn cancel(self) {
        self.cancel.cancel();
    }
}
