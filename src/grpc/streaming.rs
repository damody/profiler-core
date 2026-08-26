use crossbeam_queue::ArrayQueue;
use prost::Message;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle as ThreadJoinHandle};
use std::time::Duration;
use tokio::runtime::Runtime;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::proto::{
    CmlDataPoint, CmlStreamRequest, CrDataPoint, CrStreamRequest, GcDataPoint, GcStreamRequest,
    RtbDataPoint, RtbStreamRequest, TcDataPoint, TcStreamRequest,
};

/// A handle to an active RTB gRPC stream.
///
/// Data points are pushed into a bounded lock-free queue by a background
/// tokio task.  The FFI layer polls the queue non-blocking.
pub enum RtbStreamHandle {
    Grpc(GrpcRtbStreamHandle),
    Sync(SyncRtbStreamHandle),
}

const SYNC_CMD_START_RTB_STREAM: u8 = 1;
const SYNC_CMD_START_CR_STREAM: u8 = 38;
const SYNC_CMD_START_TC_STREAM: u8 = 39;
const SYNC_CMD_START_CML_STREAM: u8 = 40;
const SYNC_CMD_START_GC_STREAM: u8 = 41;

pub struct GrpcRtbStreamHandle {
    queue: Arc<ArrayQueue<RtbDataPoint>>,
    cancel: CancellationToken,
    _task: JoinHandle<()>,
}

pub struct SyncRtbStreamHandle {
    queue: Arc<ArrayQueue<RtbDataPoint>>,
    cancel: Arc<AtomicBool>,
    shutdown_stream: TcpStream,
    thread: ThreadJoinHandle<()>,
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

        Self::Grpc(GrpcRtbStreamHandle {
            queue,
            cancel,
            _task: task,
        })
    }

    pub fn start_sync(
        addr: &str,
        request: RtbStreamRequest,
        capacity: usize,
    ) -> anyhow::Result<Self> {
        SyncRtbStreamHandle::start(addr, request, capacity).map(Self::Sync)
    }

    /// Non-blocking poll: returns the next data point if available.
    pub fn poll(&self) -> Option<RtbDataPoint> {
        match self {
            Self::Grpc(handle) => handle.queue.pop(),
            Self::Sync(handle) => handle.queue.pop(),
        }
    }

    /// Cancel the background stream task.
    pub fn cancel(self) {
        match self {
            Self::Grpc(handle) => {
                handle.cancel.cancel();
                // The JoinHandle is dropped, which is fine – the task will
                // notice the cancellation token and exit.
            }
            Self::Sync(handle) => handle.cancel(),
        }
    }
}

impl SyncRtbStreamHandle {
    fn start(addr: &str, request: RtbStreamRequest, capacity: usize) -> anyhow::Result<Self> {
        let socket_addr = resolve_socket_addr(addr)?;
        let mut stream = TcpStream::connect_timeout(&socket_addr, Duration::from_secs(2))?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;

        let request_bytes = request.encode_to_vec();
        stream.write_all(&[SYNC_CMD_START_RTB_STREAM])?;
        stream.write_all(&(request_bytes.len() as u32).to_be_bytes())?;
        stream.write_all(&request_bytes)?;
        stream.flush()?;

        let mut status = [0u8; 1];
        stream.read_exact(&mut status)?;
        if status[0] != 0 {
            let message = read_sync_error_message(&mut stream)?;
            anyhow::bail!("sync RTB start rejected: {message}");
        }

        stream.set_read_timeout(Some(Duration::from_millis(250)))?;
        let mut reader = stream.try_clone()?;
        let shutdown_stream = stream.try_clone()?;
        let queue = Arc::new(ArrayQueue::new(capacity));
        let cancel = Arc::new(AtomicBool::new(false));
        let q = queue.clone();
        let ct = cancel.clone();

        let thread = thread::Builder::new()
            .name("rtb-sync-client".to_string())
            .spawn(move || {
                while !ct.load(Ordering::Acquire) {
                    match read_sync_rtb_point(&mut reader) {
                        Ok(Some(dp)) => match q.push(dp) {
                            Ok(()) => {}
                            Err(rejected) => {
                                let _ = q.pop();
                                let _ = q.push(rejected);
                                log::trace!("sync RTB queue full, dropped oldest sample");
                            }
                        },
                        Ok(None) => break,
                        Err(e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                || e.kind() == std::io::ErrorKind::TimedOut => {}
                        Err(e) => {
                            log::warn!("sync RTB read failed: {e}");
                            break;
                        }
                    }
                }
            })?;

        Ok(Self {
            queue,
            cancel,
            shutdown_stream,
            thread,
        })
    }

    fn cancel(self) {
        self.cancel.store(true, Ordering::Release);
        let _ = self.shutdown_stream.shutdown(Shutdown::Both);
        let _ = self.thread.join();
    }
}

fn resolve_socket_addr(addr: &str) -> anyhow::Result<SocketAddr> {
    addr.to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow::anyhow!("invalid sync RTB address: {addr}"))
}

fn read_sync_error_message(stream: &mut TcpStream) -> anyhow::Result<String> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 64 * 1024 {
        anyhow::bail!("sync RTB error message too large: {len}");
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn read_sync_rtb_point(stream: &mut TcpStream) -> std::io::Result<Option<RtbDataPoint>> {
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e)
            if e.kind() == std::io::ErrorKind::UnexpectedEof
                || e.kind() == std::io::ErrorKind::ConnectionReset =>
        {
            return Ok(None)
        }
        Err(e) => return Err(e),
    }

    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 || len > 1024 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid sync RTB frame length: {len}"),
        ));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    RtbDataPoint::decode(&buf[..])
        .map(Some)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

pub struct SyncStreamHandle<T> {
    queue: Arc<ArrayQueue<T>>,
    cancel: Arc<AtomicBool>,
    shutdown_stream: TcpStream,
    thread: ThreadJoinHandle<()>,
    finished: Arc<AtomicBool>,
}

impl<T> SyncStreamHandle<T>
where
    T: Message + Default + Send + 'static,
{
    fn start<R>(
        addr: &str,
        opcode: u8,
        request: R,
        capacity: usize,
        thread_name: &'static str,
        label: &'static str,
    ) -> anyhow::Result<Self>
    where
        R: Message,
    {
        let socket_addr = resolve_socket_addr(addr)?;
        let mut stream = TcpStream::connect_timeout(&socket_addr, Duration::from_secs(2))?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_secs(5)))?;

        let request_bytes = request.encode_to_vec();
        stream.write_all(&[opcode])?;
        stream.write_all(&(request_bytes.len() as u32).to_be_bytes())?;
        stream.write_all(&request_bytes)?;
        stream.flush()?;

        let mut status = [0u8; 1];
        stream.read_exact(&mut status)?;
        if status[0] != 0 {
            let message = read_sync_error_message(&mut stream)?;
            anyhow::bail!("sync {label} start rejected: {message}");
        }

        stream.set_read_timeout(Some(Duration::from_millis(250)))?;
        let mut reader = stream.try_clone()?;
        let shutdown_stream = stream.try_clone()?;
        let queue = Arc::new(ArrayQueue::new(capacity));
        let cancel = Arc::new(AtomicBool::new(false));
        let finished = Arc::new(AtomicBool::new(false));
        let q = queue.clone();
        let ct = cancel.clone();
        let fin = finished.clone();

        let thread = thread::Builder::new()
            .name(thread_name.to_string())
            .spawn(move || {
                while !ct.load(Ordering::Acquire) {
                    match read_sync_point::<T>(&mut reader) {
                        Ok(Some(dp)) => match q.push(dp) {
                            Ok(()) => {}
                            Err(rejected) => {
                                let _ = q.pop();
                                let _ = q.push(rejected);
                                log::trace!("sync {label} queue full, dropped oldest sample");
                            }
                        },
                        Ok(None) => break,
                        Err(e)
                            if e.kind() == std::io::ErrorKind::WouldBlock
                                || e.kind() == std::io::ErrorKind::TimedOut => {}
                        Err(e) => {
                            log::warn!("sync {label} read failed: {e}");
                            break;
                        }
                    }
                }
                fin.store(true, Ordering::Release);
            })?;

        Ok(Self {
            queue,
            cancel,
            shutdown_stream,
            thread,
            finished,
        })
    }

    fn poll(&self) -> Option<T> {
        self.queue.pop()
    }

    fn is_finished(&self) -> bool {
        self.finished.load(Ordering::Acquire)
    }

    fn cancel(self) {
        self.cancel.store(true, Ordering::Release);
        let _ = self.shutdown_stream.shutdown(Shutdown::Both);
        let _ = self.thread.join();
    }
}

fn read_sync_point<T>(stream: &mut TcpStream) -> std::io::Result<Option<T>>
where
    T: Message + Default,
{
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e)
            if e.kind() == std::io::ErrorKind::UnexpectedEof
                || e.kind() == std::io::ErrorKind::ConnectionReset =>
        {
            return Ok(None);
        }
        Err(e) => return Err(e),
    }

    let len = u32::from_be_bytes(len_buf) as usize;
    if len == 0 || len > 1024 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("invalid sync stream frame length: {len}"),
        ));
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    T::decode(&buf[..])
        .map(Some)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// A handle to an active CR (Cache Report) gRPC stream.
///
/// Data points are pushed into a bounded lock-free queue by a background
/// tokio task.  The FFI layer polls the queue non-blocking.
pub enum CrStreamHandle {
    Grpc(GrpcCrStreamHandle),
    Sync(SyncStreamHandle<CrDataPoint>),
}

pub struct GrpcCrStreamHandle {
    queue: Arc<ArrayQueue<CrDataPoint>>,
    cancel: CancellationToken,
    _task: JoinHandle<()>,
}

impl CrStreamHandle {
    /// Spawn a background task that reads from the gRPC stream and pushes
    /// data points into the internal queue.
    pub fn start(rt: &Runtime, mut stream: tonic::Streaming<CrDataPoint>, capacity: usize) -> Self {
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

        Self::Grpc(GrpcCrStreamHandle {
            queue,
            cancel,
            _task: task,
        })
    }

    pub fn start_sync(
        addr: &str,
        request: CrStreamRequest,
        capacity: usize,
    ) -> anyhow::Result<Self> {
        SyncStreamHandle::start(
            addr,
            SYNC_CMD_START_CR_STREAM,
            request,
            capacity,
            "cr-sync-client",
            "CR",
        )
        .map(Self::Sync)
    }

    /// Non-blocking poll: returns the next data point if available.
    pub fn poll(&self) -> Option<CrDataPoint> {
        match self {
            Self::Grpc(handle) => handle.queue.pop(),
            Self::Sync(handle) => handle.poll(),
        }
    }

    /// Cancel the background stream task.
    pub fn cancel(self) {
        match self {
            Self::Grpc(handle) => handle.cancel.cancel(),
            Self::Sync(handle) => handle.cancel(),
        }
    }
}

/// A handle to an active TC (Thread Cache) gRPC stream.
///
/// Data points are pushed into a bounded lock-free queue by a background
/// tokio task.  The FFI layer polls the queue non-blocking.
pub enum TcStreamHandle {
    Grpc(GrpcTcStreamHandle),
    Sync(SyncStreamHandle<TcDataPoint>),
}

pub struct GrpcTcStreamHandle {
    queue: Arc<ArrayQueue<TcDataPoint>>,
    cancel: CancellationToken,
    _task: JoinHandle<()>,
}

impl TcStreamHandle {
    /// Spawn a background task that reads from the gRPC stream and pushes
    /// data points into the internal queue.
    pub fn start(rt: &Runtime, mut stream: tonic::Streaming<TcDataPoint>, capacity: usize) -> Self {
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

        Self::Grpc(GrpcTcStreamHandle {
            queue,
            cancel,
            _task: task,
        })
    }

    pub fn start_sync(
        addr: &str,
        request: TcStreamRequest,
        capacity: usize,
    ) -> anyhow::Result<Self> {
        SyncStreamHandle::start(
            addr,
            SYNC_CMD_START_TC_STREAM,
            request,
            capacity,
            "tc-sync-client",
            "TC",
        )
        .map(Self::Sync)
    }

    /// Non-blocking poll: returns the next data point if available.
    pub fn poll(&self) -> Option<TcDataPoint> {
        match self {
            Self::Grpc(handle) => handle.queue.pop(),
            Self::Sync(handle) => handle.poll(),
        }
    }

    /// Cancel the background stream task.
    pub fn cancel(self) {
        match self {
            Self::Grpc(handle) => handle.cancel.cancel(),
            Self::Sync(handle) => handle.cancel(),
        }
    }
}

/// A handle to an active CML (Cache/Memory Latency) gRPC stream.
///
/// CML is a finite-length benchmark (typically ~17 data points).
/// The background task sets `is_finished` when the stream ends naturally.
pub enum CmlStreamHandle {
    Grpc(GrpcCmlStreamHandle),
    Sync(SyncStreamHandle<CmlDataPoint>),
}

pub struct GrpcCmlStreamHandle {
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

        Self::Grpc(GrpcCmlStreamHandle {
            queue,
            cancel,
            _task: task,
            finished,
        })
    }

    pub fn start_sync(
        addr: &str,
        request: CmlStreamRequest,
        capacity: usize,
    ) -> anyhow::Result<Self> {
        SyncStreamHandle::start(
            addr,
            SYNC_CMD_START_CML_STREAM,
            request,
            capacity,
            "cml-sync-client",
            "CML",
        )
        .map(Self::Sync)
    }

    /// Non-blocking poll: returns the next data point if available.
    pub fn poll(&self) -> Option<CmlDataPoint> {
        match self {
            Self::Grpc(handle) => handle.queue.pop(),
            Self::Sync(handle) => handle.poll(),
        }
    }

    /// Returns true if the background stream task has finished
    /// (benchmark complete or error).
    pub fn is_finished(&self) -> bool {
        match self {
            Self::Grpc(handle) => handle.finished.load(Ordering::Acquire),
            Self::Sync(handle) => handle.is_finished(),
        }
    }

    /// Cancel the background stream task.
    pub fn cancel(self) {
        match self {
            Self::Grpc(handle) => handle.cancel.cancel(),
            Self::Sync(handle) => handle.cancel(),
        }
    }
}

/// A handle to an active GC (GPU Counters) gRPC stream.
///
/// Data points are pushed into a bounded lock-free queue by a background
/// tokio task.  The FFI layer polls the queue non-blocking.
pub enum GcStreamHandle {
    Grpc(GrpcGcStreamHandle),
    Sync(SyncStreamHandle<GcDataPoint>),
}

pub struct GrpcGcStreamHandle {
    queue: Arc<ArrayQueue<GcDataPoint>>,
    cancel: CancellationToken,
    _task: JoinHandle<()>,
}

impl GcStreamHandle {
    /// Spawn a background task that reads from the gRPC stream and pushes
    /// data points into the internal queue.
    pub fn start(rt: &Runtime, mut stream: tonic::Streaming<GcDataPoint>, capacity: usize) -> Self {
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

        Self::Grpc(GrpcGcStreamHandle {
            queue,
            cancel,
            _task: task,
        })
    }

    pub fn start_sync(
        addr: &str,
        request: GcStreamRequest,
        capacity: usize,
    ) -> anyhow::Result<Self> {
        SyncStreamHandle::start(
            addr,
            SYNC_CMD_START_GC_STREAM,
            request,
            capacity,
            "gc-sync-client",
            "GC",
        )
        .map(Self::Sync)
    }

    /// Non-blocking poll: returns the next data point if available.
    pub fn poll(&self) -> Option<GcDataPoint> {
        match self {
            Self::Grpc(handle) => handle.queue.pop(),
            Self::Sync(handle) => handle.poll(),
        }
    }

    /// Cancel the background stream task.
    pub fn cancel(self) {
        match self {
            Self::Grpc(handle) => handle.cancel.cancel(),
            Self::Sync(handle) => handle.cancel(),
        }
    }
}

#[cfg(test)]
mod gpu_hardware_tests {
    use super::*;
    use std::time::Instant;

    #[test]
    #[ignore = "requires ANDROID_PROFILER_GPU_SYNC_SMOKE=1 and an adb-forwarded sync daemon"]
    fn connected_sync_gpu_counter_stream_returns_values() {
        if std::env::var_os("ANDROID_PROFILER_GPU_SYNC_SMOKE").as_deref() != Some("1".as_ref()) {
            return;
        }
        let addr = std::env::var("ANDROID_PROFILER_GPU_SYNC_ADDR")
            .unwrap_or_else(|_| "127.0.0.1:50552".to_string());
        let counter_ids = std::env::var("ANDROID_PROFILER_GPU_COUNTER_IDS")
            .ok()
            .map(|value| {
                value
                    .split(',')
                    .filter_map(|part| part.trim().parse().ok())
                    .collect::<Vec<_>>()
            })
            .filter(|ids| !ids.is_empty())
            .unwrap_or_else(|| vec![80]);
        let handle = GcStreamHandle::start_sync(
            &addr,
            GcStreamRequest {
                gpu_device_number: 0,
                interval_secs: 0.25,
                counter_ids: counter_ids.clone(),
            },
            64,
        )
        .expect("start direct sync GPU stream");

        let deadline = Instant::now() + Duration::from_secs(15);
        let mut last_point = None;
        let point = loop {
            if let Some(point) = handle.poll() {
                eprintln!("direct GPU counter evidence: {:?}", point.counters);
                if point
                    .counters
                    .iter()
                    .any(|counter| counter.value.is_finite() && counter.value != 0.0)
                {
                    break point;
                }
                last_point = Some(point);
            }
            assert!(
                Instant::now() < deadline,
                "GPU stream produced no non-zero samples; last={last_point:?}"
            );
            std::thread::sleep(Duration::from_millis(25));
        };
        assert_eq!(point.counters.len(), counter_ids.len());
        assert!(point.counters.iter().all(|counter| {
            counter_ids.contains(&counter.counter_id) && counter.value.is_finite()
        }));
        handle.cancel();
    }
}
