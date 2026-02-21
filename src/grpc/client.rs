use anyhow::{Context, Result};
use std::io::Write;
use tonic::transport::Channel;

use crate::proto::profiler_service_client::ProfilerServiceClient;
use crate::proto::{
    ChmodRequest, ChownRequest, CmlStreamRequest, CrStreamRequest, CreateArchiveRequest,
    DevicePropRequest, Empty, ExtractArchiveRequest, FileUploadChunk, GetFileOwnerRequest,
    GetPidRequest, GetSurfaceNamesRequest, InputKeyEventRequest, InputSwipeRequest,
    InputTapRequest, InputTextRequest, InstallApkRequest, ListPackagesRequest, PackageRequest,
    PathExistsRequest, PerfettoRequest, PerfettoResponse, PullFileRequest, RemoveFileRequest,
    RtbStreamRequest, RtbSummaryRequest, ScreenshotRequest, SetChargingRequest, ShellRequest,
    StopResponse, TcStreamRequest,
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

/// Package info returned by gRPC.
pub struct PackageInfoResult {
    pub package_name: String,
    pub apk_path: String,
    pub version_name: String,
    pub version_code: i32,
    pub pid: i32,
}

/// Shell command result.
pub struct ShellResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Generic response from the daemon.
pub struct GenericResult {
    pub success: bool,
    pub message: String,
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
        mode: &str,
    ) -> Result<tonic::Streaming<crate::proto::RtbDataPoint>> {
        let resp = self
            .inner
            .start_rtb_stream(RtbStreamRequest {
                pid,
                interval_secs,
                mode: mode.to_string(),
            })
            .await
            .context("StartRtbStream RPC failed")?;
        Ok(resp.into_inner())
    }

    /// Start a Cache Report stream and return the tonic streaming response.
    pub async fn start_cr_stream(
        &mut self,
        interval_secs: f64,
        cpus: &[i32],
        exclude_kernel: bool,
        diff_kernel: bool,
        full_mode: bool,
        custom_events: &[u32],
    ) -> Result<tonic::Streaming<crate::proto::CrDataPoint>> {
        let resp = self
            .inner
            .start_cr_stream(CrStreamRequest {
                interval_secs,
                cpus: cpus.to_vec(),
                exclude_kernel,
                full_mode,
                custom_events: custom_events.to_vec(),
                diff_kernel,
            })
            .await
            .context("StartCrStream RPC failed")?;
        Ok(resp.into_inner())
    }

    /// Start a CML (Cache/Memory Latency) benchmark stream.
    pub async fn start_cml_stream(
        &mut self,
        cpus: &[i32],
        max_footprint_kb: u32,
        min_footprint_kb: u32,
    ) -> Result<tonic::Streaming<crate::proto::CmlDataPoint>> {
        let resp = self
            .inner
            .start_cml_stream(CmlStreamRequest {
                cpus: cpus.to_vec(),
                max_footprint_kb,
                min_footprint_kb,
            })
            .await
            .context("StartCmlStream RPC failed")?;
        Ok(resp.into_inner())
    }

    /// Start a Thread Cache stream and return the tonic streaming response.
    pub async fn start_tc_stream(
        &mut self,
        pid: i32,
        interval_secs: f64,
        exclude_kernel: bool,
        diff_kernel: bool,
        top_threads_count: i32,
    ) -> Result<tonic::Streaming<crate::proto::TcDataPoint>> {
        let resp = self
            .inner
            .start_tc_stream(TcStreamRequest {
                pid,
                interval_secs,
                exclude_kernel,
                diff_kernel,
                top_threads_count,
            })
            .await
            .context("StartTcStream RPC failed")?;
        Ok(resp.into_inner())
    }

    /// Start a Perfetto trace session.
    pub async fn start_perfetto(
        &mut self,
        pid: i32,
        mode: &str,
        duration_secs: i32,
        config_pbtxt: &str,
    ) -> Result<PerfettoResponse> {
        let resp = self
            .inner
            .start_perfetto(PerfettoRequest {
                pid,
                mode: mode.to_string(),
                duration_secs,
                config_pbtxt: config_pbtxt.to_string(),
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

    // =========================================================================
    // Package Management
    // =========================================================================

    /// List installed packages.
    pub async fn list_packages(&mut self, third_party_only: bool) -> Result<Vec<PackageInfoResult>> {
        let resp = self
            .inner
            .list_packages(ListPackagesRequest { third_party_only })
            .await
            .context("ListPackages RPC failed")?
            .into_inner();

        Ok(resp
            .packages
            .into_iter()
            .map(|p| PackageInfoResult {
                package_name: p.package_name,
                apk_path: p.apk_path,
                version_name: p.version_name,
                version_code: p.version_code,
                pid: p.pid,
            })
            .collect())
    }

    /// Get detailed info for a single package.
    pub async fn get_package_info(&mut self, package_name: &str) -> Result<PackageInfoResult> {
        let resp = self
            .inner
            .get_package_info(PackageRequest {
                package_name: package_name.to_string(),
            })
            .await
            .context("GetPackageInfo RPC failed")?
            .into_inner();

        Ok(PackageInfoResult {
            package_name: resp.package_name,
            apk_path: resp.apk_path,
            version_name: resp.version_name,
            version_code: resp.version_code,
            pid: resp.pid,
        })
    }

    /// Launch an app by package name.
    pub async fn launch_app(&mut self, package_name: &str) -> Result<GenericResult> {
        let resp = self
            .inner
            .launch_app(PackageRequest {
                package_name: package_name.to_string(),
            })
            .await
            .context("LaunchApp RPC failed")?
            .into_inner();

        Ok(GenericResult {
            success: resp.success,
            message: resp.message,
        })
    }

    /// Force-stop an app by package name.
    pub async fn stop_app(&mut self, package_name: &str) -> Result<GenericResult> {
        let resp = self
            .inner
            .stop_app(PackageRequest {
                package_name: package_name.to_string(),
            })
            .await
            .context("StopApp RPC failed")?
            .into_inner();

        Ok(GenericResult {
            success: resp.success,
            message: resp.message,
        })
    }

    /// Install an APK already on the device.
    pub async fn install_apk(&mut self, remote_apk_path: &str) -> Result<GenericResult> {
        let resp = self
            .inner
            .install_apk(InstallApkRequest {
                remote_apk_path: remote_apk_path.to_string(),
            })
            .await
            .context("InstallApk RPC failed")?
            .into_inner();

        Ok(GenericResult {
            success: resp.success,
            message: resp.message,
        })
    }

    // =========================================================================
    // Shell
    // =========================================================================

    /// Execute a shell command on the device (daemon already has root).
    pub async fn shell(&mut self, command: &str) -> Result<ShellResult> {
        let resp = self
            .inner
            .shell(ShellRequest {
                command: command.to_string(),
            })
            .await
            .context("Shell RPC failed")?
            .into_inner();

        Ok(ShellResult {
            exit_code: resp.exit_code,
            stdout: resp.stdout,
            stderr: resp.stderr,
        })
    }

    // =========================================================================
    // Screen & Input
    // =========================================================================

    /// Get the device screen size.
    pub async fn get_screen_size(&mut self) -> Result<(i32, i32)> {
        let resp = self
            .inner
            .get_screen_size(Empty {})
            .await
            .context("GetScreenSize RPC failed")?
            .into_inner();

        Ok((resp.width, resp.height))
    }

    /// Take a screenshot, streaming JPEG bytes to a local file.
    pub async fn screenshot<F>(
        &mut self,
        quality: i32,
        local_path: &str,
        progress_cb: F,
    ) -> Result<()>
    where
        F: Fn(u64, u64),
    {
        let mut stream = self
            .inner
            .screenshot(ScreenshotRequest { quality })
            .await
            .context("Screenshot RPC failed")?
            .into_inner();

        let mut file = std::fs::File::create(local_path)
            .context("Failed to create screenshot output file")?;

        let mut bytes_received: u64 = 0;

        while let Some(chunk) = stream.message().await.context("Error reading screenshot chunk")? {
            file.write_all(&chunk.data)
                .context("Failed to write screenshot chunk")?;
            bytes_received += chunk.data.len() as u64;
            progress_cb(bytes_received, 0);

            if chunk.is_last {
                break;
            }
        }

        file.flush().context("Failed to flush screenshot file")?;
        progress_cb(bytes_received, bytes_received);

        log::info!("Screenshot saved to {local_path} ({bytes_received} bytes)");
        Ok(())
    }

    /// Tap at the given coordinates.
    pub async fn input_tap(&mut self, x: i32, y: i32) -> Result<GenericResult> {
        let resp = self
            .inner
            .input_tap(InputTapRequest { x, y })
            .await
            .context("InputTap RPC failed")?
            .into_inner();

        Ok(GenericResult {
            success: resp.success,
            message: resp.message,
        })
    }

    /// Swipe between two points.
    pub async fn input_swipe(
        &mut self,
        x1: i32,
        y1: i32,
        x2: i32,
        y2: i32,
        duration_ms: i32,
    ) -> Result<GenericResult> {
        let resp = self
            .inner
            .input_swipe(InputSwipeRequest {
                x1,
                y1,
                x2,
                y2,
                duration_ms,
            })
            .await
            .context("InputSwipe RPC failed")?
            .into_inner();

        Ok(GenericResult {
            success: resp.success,
            message: resp.message,
        })
    }

    /// Input text on the device.
    pub async fn input_text(&mut self, text: &str) -> Result<GenericResult> {
        let resp = self
            .inner
            .input_text(InputTextRequest {
                text: text.to_string(),
            })
            .await
            .context("InputText RPC failed")?
            .into_inner();

        Ok(GenericResult {
            success: resp.success,
            message: resp.message,
        })
    }

    /// Send a key event.
    pub async fn input_key_event(&mut self, key_code: i32) -> Result<GenericResult> {
        let resp = self
            .inner
            .input_key_event(InputKeyEventRequest { key_code })
            .await
            .context("InputKeyEvent RPC failed")?
            .into_inner();

        Ok(GenericResult {
            success: resp.success,
            message: resp.message,
        })
    }

    // =========================================================================
    // File Push (client streaming)
    // =========================================================================

    /// Push a local file to the device via client streaming.
    pub async fn push_file<F>(
        &mut self,
        local_path: &str,
        remote_path: &str,
        progress_cb: F,
    ) -> Result<u64>
    where
        F: Fn(u64, u64),
    {
        let file_size = std::fs::metadata(local_path)
            .context("Failed to read local file metadata")?
            .len();

        let file = std::fs::File::open(local_path).context("Failed to open local file")?;
        let mut reader = std::io::BufReader::new(file);

        let remote = remote_path.to_string();
        let mut bytes_sent: u64 = 0;
        let mut chunks = Vec::new();
        let chunk_size: usize = 64 * 1024; // 64KB chunks

        loop {
            let mut buf = vec![0u8; chunk_size];
            let n = std::io::Read::read(&mut reader, &mut buf)
                .context("Failed to read local file")?;
            if n == 0 {
                break;
            }
            buf.truncate(n);
            bytes_sent += n as u64;
            let is_last = bytes_sent >= file_size;

            chunks.push(FileUploadChunk {
                remote_path: if chunks.is_empty() { remote.clone() } else { String::new() },
                data: buf,
                is_last,
            });

            progress_cb(bytes_sent, file_size);

            if is_last {
                break;
            }
        }

        let resp = self
            .inner
            .push_file(tokio_stream::iter(chunks))
            .await
            .context("PushFile RPC failed")?
            .into_inner();

        if !resp.success {
            anyhow::bail!("PushFile failed on server side");
        }

        log::info!(
            "Pushed {} -> {} ({} bytes)",
            local_path,
            remote_path,
            resp.bytes_written
        );
        Ok(resp.bytes_written)
    }

    // =========================================================================
    // Device Utilities
    // =========================================================================

    /// Get a device property value (e.g. "ro.hardware").
    pub async fn get_device_prop(&mut self, prop_name: &str) -> Result<String> {
        let resp = self
            .inner
            .get_device_prop(DevicePropRequest {
                prop_name: prop_name.to_string(),
            })
            .await
            .context("GetDeviceProp RPC failed")?
            .into_inner();

        Ok(resp.value)
    }

    /// Check if a path exists on the device.
    pub async fn path_exists(&mut self, path: &str) -> Result<bool> {
        let resp = self
            .inner
            .path_exists(PathExistsRequest {
                path: path.to_string(),
            })
            .await
            .context("PathExists RPC failed")?
            .into_inner();

        Ok(resp.exists)
    }

    /// Get battery and board temperatures and battery level from the device.
    pub async fn get_temperature(&mut self) -> Result<(f64, f64, u32)> {
        let resp = self
            .inner
            .get_temperature(Empty {})
            .await
            .context("GetTemperature RPC failed")?
            .into_inner();
        Ok((resp.battery_temp_c, resp.board_temp_c, resp.battery_level_pct))
    }

    // =========================================================================
    // RTB Summary
    // =========================================================================

    /// Get the RTB summary (post-recording statistics).
    pub async fn get_rtb_summary(&mut self) -> Result<crate::proto::RtbSummary> {
        let resp = self
            .inner
            .get_rtb_summary(RtbSummaryRequest {})
            .await
            .context("GetRtbSummary RPC failed")?
            .into_inner();
        Ok(resp)
    }

    // =========================================================================
    // Surface Names
    // =========================================================================

    /// Get SurfaceFlinger surface names matching a package.
    pub async fn get_surface_names(&mut self, package_name: &str) -> Result<Vec<String>> {
        let resp = self
            .inner
            .get_surface_names(GetSurfaceNamesRequest {
                package_name: package_name.to_string(),
            })
            .await
            .context("GetSurfaceNames RPC failed")?
            .into_inner();
        Ok(resp.surface_names)
    }

    // =========================================================================
    // Charging Control
    // =========================================================================

    /// Enable or disable charging.
    pub async fn set_charging(&mut self, enable: bool) -> Result<GenericResult> {
        let resp = self
            .inner
            .set_charging(SetChargingRequest { enable })
            .await
            .context("SetCharging RPC failed")?
            .into_inner();
        Ok(GenericResult {
            success: resp.success,
            message: resp.message,
        })
    }

    // =========================================================================
    // File Operations
    // =========================================================================

    /// Remove a file on the device.
    pub async fn remove_file(&mut self, path: &str) -> Result<GenericResult> {
        let resp = self
            .inner
            .remove_file(RemoveFileRequest {
                path: path.to_string(),
            })
            .await
            .context("RemoveFile RPC failed")?
            .into_inner();
        Ok(GenericResult {
            success: resp.success,
            message: resp.message,
        })
    }

    /// Create a tar.gz archive on the device.
    pub async fn create_archive(
        &mut self,
        working_directory: &str,
        target: &str,
        output_path: &str,
    ) -> Result<GenericResult> {
        let resp = self
            .inner
            .create_archive(CreateArchiveRequest {
                working_directory: working_directory.to_string(),
                target: target.to_string(),
                output_path: output_path.to_string(),
            })
            .await
            .context("CreateArchive RPC failed")?
            .into_inner();
        Ok(GenericResult {
            success: resp.success,
            message: resp.message,
        })
    }

    /// Extract a tar.gz archive on the device.
    pub async fn extract_archive(
        &mut self,
        working_directory: &str,
        archive_path: &str,
    ) -> Result<GenericResult> {
        let resp = self
            .inner
            .extract_archive(ExtractArchiveRequest {
                working_directory: working_directory.to_string(),
                archive_path: archive_path.to_string(),
            })
            .await
            .context("ExtractArchive RPC failed")?
            .into_inner();
        Ok(GenericResult {
            success: resp.success,
            message: resp.message,
        })
    }

    /// Change file permissions on the device.
    pub async fn chmod(&mut self, path: &str, mode: &str, recursive: bool) -> Result<GenericResult> {
        let resp = self
            .inner
            .chmod(ChmodRequest {
                path: path.to_string(),
                mode: mode.to_string(),
                recursive,
            })
            .await
            .context("Chmod RPC failed")?
            .into_inner();
        Ok(GenericResult {
            success: resp.success,
            message: resp.message,
        })
    }

    /// Change file ownership on the device.
    pub async fn chown(&mut self, path: &str, uid: i32, gid: i32, recursive: bool) -> Result<GenericResult> {
        let resp = self
            .inner
            .chown(ChownRequest {
                path: path.to_string(),
                uid,
                gid,
                recursive,
            })
            .await
            .context("Chown RPC failed")?
            .into_inner();
        Ok(GenericResult {
            success: resp.success,
            message: resp.message,
        })
    }

    /// Get the UID owner of a file on the device.
    pub async fn get_file_owner(&mut self, path: &str) -> Result<i32> {
        let resp = self
            .inner
            .get_file_owner(GetFileOwnerRequest {
                path: path.to_string(),
            })
            .await
            .context("GetFileOwner RPC failed")?
            .into_inner();
        Ok(resp.uid)
    }
}
