use anyhow::{Context, Result};
use std::io::Write;
use tonic::transport::Channel;

use crate::proto::profiler_service_client::ProfilerServiceClient;
use crate::proto::{
    Empty, GetPidRequest, PerfettoRequest, PerfettoResponse, PullFileRequest,
    RtbStreamRequest, StopResponse,
};

/// High-level wrapper around the gRPC ProfilerServiceClient.
pub struct ProfilerClient {
    inner: ProfilerServiceClient<Channel>,
}

/// Perfetto status returned by get_perfetto_status.
pub struct PerfettoStatusInfo {
    pub state: i32,
    pub progress_pct: f64,
    pub output_path: String,
}

/// Top app info returned by get_top_app.
pub struct TopAppInfo {
    pub package_name: String,
    pub activity: String,
    pub pid: i32,
}

impl ProfilerClient {
    /// Connect to the gRPC server at the given address (e.g. "http://127.0.0.1:50051").
    pub async fn connect(addr: &str) -> Result<Self> {
        let channel = Channel::from_shared(addr.to_string())
            .context("Invalid gRPC address")?
            .connect_timeout(std::time::Duration::from_secs(5))
            .connect()
            .await
            .context("Failed to connect to gRPC server")?;

        let client = ProfilerServiceClient::new(channel);
        Ok(Self { inner: client })
    }

    /// Health check.
    pub async fn health(&mut self) -> Result<(String, String)> {
        let resp = self
            .inner
            .health(Empty {})
            .await
            .context("Health RPC failed")?
            .into_inner();
        Ok((resp.version, resp.status))
    }

    /// Get the foreground app info.
    pub async fn get_top_app(&mut self) -> Result<TopAppInfo> {
        let resp = self
            .inner
            .get_top_app(Empty {})
            .await
            .context("GetTopApp RPC failed")?
            .into_inner();
        Ok(TopAppInfo {
            package_name: resp.package_name,
            activity: resp.activity,
            pid: resp.pid,
        })
    }

    /// Get the PID of a package.
    pub async fn get_pid(&mut self, package_name: &str) -> Result<i32> {
        let resp = self
            .inner
            .get_pid(GetPidRequest {
                package_name: package_name.to_string(),
            })
            .await
            .context("GetPid RPC failed")?
            .into_inner();
        Ok(resp.pid)
    }

    /// Start an RTB stream and return the tonic streaming response.
    pub async fn start_rtb_stream(
        &mut self,
        pid: i32,
        interval_secs: f64,
    ) -> Result<tonic::Streaming<crate::proto::RtbDataPoint>> {
        let resp = self
            .inner
            .start_rtb_stream(RtbStreamRequest {
                pid,
                interval_secs,
            })
            .await
            .context("StartRtbStream RPC failed")?;
        Ok(resp.into_inner())
    }

    /// Start a Perfetto trace session.
    pub async fn start_perfetto(
        &mut self,
        pid: i32,
        mode: &str,
        duration_secs: i32,
    ) -> Result<PerfettoResponse> {
        let resp = self
            .inner
            .start_perfetto(PerfettoRequest {
                pid,
                mode: mode.to_string(),
                duration_secs,
            })
            .await
            .context("StartPerfetto RPC failed")?
            .into_inner();
        Ok(resp)
    }

    /// Get the current Perfetto status.
    pub async fn get_perfetto_status(&mut self) -> Result<PerfettoStatusInfo> {
        let resp = self
            .inner
            .get_perfetto_status(Empty {})
            .await
            .context("GetPerfettoStatus RPC failed")?
            .into_inner();
        Ok(PerfettoStatusInfo {
            state: resp.state,
            progress_pct: resp.progress_pct,
            output_path: resp.output_path,
        })
    }

    /// Pull a file from the device, writing to a local path.
    ///
    /// The `progress_cb` is called with (bytes_received, total_bytes).
    /// Since the server streams chunks and does not send total size upfront,
    /// `total_bytes` is set to 0 (unknown) and only `bytes_received` grows.
    pub async fn pull_file<F>(
        &mut self,
        remote_path: &str,
        local_path: &str,
        progress_cb: F,
    ) -> Result<()>
    where
        F: Fn(u64, u64),
    {
        let mut stream = self
            .inner
            .pull_file(PullFileRequest {
                remote_path: remote_path.to_string(),
            })
            .await
            .context("PullFile RPC failed")?
            .into_inner();

        let mut file = std::fs::File::create(local_path)
            .context("Failed to create local output file")?;

        let mut bytes_received: u64 = 0;

        while let Some(chunk) = stream.message().await.context("Error reading file chunk")? {
            file.write_all(&chunk.data)
                .context("Failed to write chunk to file")?;
            bytes_received += chunk.data.len() as u64;
            progress_cb(bytes_received, 0);

            if chunk.is_last {
                break;
            }
        }

        file.flush().context("Failed to flush output file")?;
        progress_cb(bytes_received, bytes_received);

        log::info!(
            "Pulled {} -> {} ({bytes_received} bytes)",
            remote_path,
            local_path
        );
        Ok(())
    }

    /// Stop the current recording on the daemon.
    pub async fn stop_recording(&mut self) -> Result<StopResponse> {
        let resp = self
            .inner
            .stop_recording(Empty {})
            .await
            .context("StopRecording RPC failed")?
            .into_inner();
        Ok(resp)
    }

    /// Shut down the daemon.
    pub async fn shutdown(&mut self) -> Result<bool> {
        let resp = self
            .inner
            .shutdown(Empty {})
            .await
            .context("Shutdown RPC failed")?
            .into_inner();
        Ok(resp.success)
    }
}
