use prost::Message;
use std::fs::File;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::time::Duration;

use crate::proto::{
    ChmodRequest, ChownRequest, CreateArchiveRequest, DevicePropRequest, DevicePropResponse,
    DiscoverFtraceResponse, DiscoverPmuResponse, Empty, ExtractArchiveRequest, FileChunk,
    FileUploadChunk, GcDiscoverRequest, GcDiscoverResponse, GenericResponse, GetFileOwnerRequest,
    GetFileOwnerResponse, GetPidRequest, GetSurfaceNamesRequest, GetSurfaceNamesResponse,
    HealthResponse, InputKeyEventRequest, InputSwipeRequest, InputTapRequest, InputTextRequest,
    InstallApkRequest, ListPackagesRequest, PackageInfo, PackageList, PackageRequest,
    PathExistsRequest, PathExistsResponse, PerfettoRequest, PerfettoResponse, PerfettoStatus,
    PullFileRequest, PushFileResponse, RemoveFileRequest, RtbSummary, RtbSummaryRequest,
    ScreenSizeResponse, ScreenshotRequest, SetChargingRequest, SetDeviceIdRequest, ShellRequest,
    ShellResponse, StopRecordingRequest, StopResponse, TemperatureResponse, TopAppInfo,
};

const SYNC_CMD_HEALTH: u8 = 2;
const SYNC_CMD_GET_PID: u8 = 3;
const SYNC_CMD_STOP_RECORDING: u8 = 4;
const SYNC_CMD_GET_RTB_SUMMARY: u8 = 5;
const SYNC_CMD_GET_SURFACE_NAMES: u8 = 6;
const SYNC_CMD_SHELL: u8 = 7;
const SYNC_CMD_PATH_EXISTS: u8 = 8;
const SYNC_CMD_GET_TOP_APP: u8 = 9;
const SYNC_CMD_GET_SCREEN_SIZE: u8 = 10;
const SYNC_CMD_INPUT_TAP: u8 = 11;
const SYNC_CMD_INPUT_SWIPE: u8 = 12;
const SYNC_CMD_INPUT_TEXT: u8 = 13;
const SYNC_CMD_INPUT_KEY_EVENT: u8 = 14;
const SYNC_CMD_LIST_PACKAGES: u8 = 15;
const SYNC_CMD_GET_PACKAGE_INFO: u8 = 16;
const SYNC_CMD_LAUNCH_APP: u8 = 17;
const SYNC_CMD_STOP_APP: u8 = 18;
const SYNC_CMD_INSTALL_APK: u8 = 19;
const SYNC_CMD_GET_DEVICE_PROP: u8 = 20;
const SYNC_CMD_GET_TEMPERATURE: u8 = 21;
const SYNC_CMD_SET_CHARGING: u8 = 22;
const SYNC_CMD_REMOVE_FILE: u8 = 23;
const SYNC_CMD_CREATE_ARCHIVE: u8 = 24;
const SYNC_CMD_EXTRACT_ARCHIVE: u8 = 25;
const SYNC_CMD_CHMOD: u8 = 26;
const SYNC_CMD_CHOWN: u8 = 27;
const SYNC_CMD_GET_FILE_OWNER: u8 = 28;
const SYNC_CMD_DISCOVER_FTRACE_EVENTS: u8 = 29;
const SYNC_CMD_DISCOVER_PMU_EVENTS: u8 = 30;
const SYNC_CMD_GET_PMU_HW_COUNTERS: u8 = 31;
const SYNC_CMD_DISCOVER_GPU_COUNTERS: u8 = 32;
const SYNC_CMD_SCREENSHOT: u8 = 33;
const SYNC_CMD_PULL_FILE: u8 = 34;
const SYNC_CMD_PUSH_FILE: u8 = 35;
const SYNC_CMD_START_PERFETTO: u8 = 36;
const SYNC_CMD_GET_PERFETTO_STATUS: u8 = 37;
const SYNC_CMD_SET_DEVICE_ID: u8 = 42;

pub fn health(addr: &str) -> anyhow::Result<(String, String)> {
    let response: HealthResponse = send_unary(addr, SYNC_CMD_HEALTH, &Empty {})?;
    Ok((response.version, response.status))
}

pub fn get_pid(addr: &str, package_name: &str) -> anyhow::Result<i32> {
    let response: crate::proto::GetPidResponse = send_unary(
        addr,
        SYNC_CMD_GET_PID,
        &GetPidRequest {
            package_name: package_name.to_string(),
        },
    )?;
    Ok(response.pid)
}

pub fn stop_recording(addr: &str, session_type: &str) -> anyhow::Result<StopResponse> {
    send_unary(
        addr,
        SYNC_CMD_STOP_RECORDING,
        &StopRecordingRequest {
            session_type: session_type.to_string(),
        },
    )
}

pub fn get_rtb_summary(addr: &str) -> anyhow::Result<RtbSummary> {
    send_unary(addr, SYNC_CMD_GET_RTB_SUMMARY, &RtbSummaryRequest {})
}

pub fn get_surface_names(addr: &str, package_name: &str) -> anyhow::Result<Vec<String>> {
    let response: GetSurfaceNamesResponse = send_unary(
        addr,
        SYNC_CMD_GET_SURFACE_NAMES,
        &GetSurfaceNamesRequest {
            package_name: package_name.to_string(),
        },
    )?;
    Ok(response.surface_names)
}

pub fn shell(addr: &str, command: &str) -> anyhow::Result<ShellResponse> {
    send_unary(
        addr,
        SYNC_CMD_SHELL,
        &ShellRequest {
            command: command.to_string(),
        },
    )
}

pub fn path_exists(addr: &str, path: &str) -> anyhow::Result<bool> {
    let response: PathExistsResponse = send_unary(
        addr,
        SYNC_CMD_PATH_EXISTS,
        &PathExistsRequest {
            path: path.to_string(),
        },
    )?;
    Ok(response.exists)
}

pub fn get_top_app(addr: &str) -> anyhow::Result<TopAppInfo> {
    send_unary(addr, SYNC_CMD_GET_TOP_APP, &Empty {})
}

pub fn get_screen_size(addr: &str) -> anyhow::Result<(i32, i32)> {
    let response: ScreenSizeResponse = send_unary(addr, SYNC_CMD_GET_SCREEN_SIZE, &Empty {})?;
    Ok((response.width, response.height))
}

pub fn input_tap(addr: &str, x: i32, y: i32) -> anyhow::Result<GenericResponse> {
    send_unary(addr, SYNC_CMD_INPUT_TAP, &InputTapRequest { x, y })
}

pub fn input_swipe(
    addr: &str,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    duration_ms: i32,
) -> anyhow::Result<GenericResponse> {
    send_unary(
        addr,
        SYNC_CMD_INPUT_SWIPE,
        &InputSwipeRequest {
            x1,
            y1,
            x2,
            y2,
            duration_ms,
        },
    )
}

pub fn input_text(addr: &str, text: &str) -> anyhow::Result<GenericResponse> {
    send_unary(
        addr,
        SYNC_CMD_INPUT_TEXT,
        &InputTextRequest {
            text: text.to_string(),
        },
    )
}

pub fn input_key_event(addr: &str, key_code: i32) -> anyhow::Result<GenericResponse> {
    send_unary(
        addr,
        SYNC_CMD_INPUT_KEY_EVENT,
        &InputKeyEventRequest { key_code },
    )
}

pub fn list_packages(addr: &str, third_party_only: bool) -> anyhow::Result<Vec<PackageInfo>> {
    let response: PackageList = send_unary(
        addr,
        SYNC_CMD_LIST_PACKAGES,
        &ListPackagesRequest { third_party_only },
    )?;
    Ok(response.packages)
}

pub fn get_package_info(addr: &str, package_name: &str) -> anyhow::Result<PackageInfo> {
    send_unary(
        addr,
        SYNC_CMD_GET_PACKAGE_INFO,
        &PackageRequest {
            package_name: package_name.to_string(),
        },
    )
}

pub fn launch_app(addr: &str, package_name: &str) -> anyhow::Result<GenericResponse> {
    send_unary(
        addr,
        SYNC_CMD_LAUNCH_APP,
        &PackageRequest {
            package_name: package_name.to_string(),
        },
    )
}

pub fn stop_app(addr: &str, package_name: &str) -> anyhow::Result<GenericResponse> {
    send_unary(
        addr,
        SYNC_CMD_STOP_APP,
        &PackageRequest {
            package_name: package_name.to_string(),
        },
    )
}

pub fn install_apk(addr: &str, remote_apk_path: &str) -> anyhow::Result<GenericResponse> {
    send_unary(
        addr,
        SYNC_CMD_INSTALL_APK,
        &InstallApkRequest {
            remote_apk_path: remote_apk_path.to_string(),
        },
    )
}

pub fn get_device_prop(addr: &str, prop_name: &str) -> anyhow::Result<String> {
    let response: DevicePropResponse = send_unary(
        addr,
        SYNC_CMD_GET_DEVICE_PROP,
        &DevicePropRequest {
            prop_name: prop_name.to_string(),
        },
    )?;
    Ok(response.value)
}

pub fn get_temperature(addr: &str) -> anyhow::Result<TemperatureResponse> {
    send_unary(addr, SYNC_CMD_GET_TEMPERATURE, &Empty {})
}

pub fn set_charging(addr: &str, enable: bool) -> anyhow::Result<GenericResponse> {
    send_unary(addr, SYNC_CMD_SET_CHARGING, &SetChargingRequest { enable })
}

pub fn set_device_id(addr: &str, device_id: &str) -> anyhow::Result<GenericResponse> {
    send_unary(
        addr,
        SYNC_CMD_SET_DEVICE_ID,
        &SetDeviceIdRequest {
            device_id: device_id.to_string(),
        },
    )
}

pub fn remove_file(addr: &str, path: &str) -> anyhow::Result<GenericResponse> {
    send_unary(
        addr,
        SYNC_CMD_REMOVE_FILE,
        &RemoveFileRequest {
            path: path.to_string(),
        },
    )
}

pub fn create_archive(
    addr: &str,
    working_directory: &str,
    target: &str,
    output_path: &str,
) -> anyhow::Result<GenericResponse> {
    send_unary(
        addr,
        SYNC_CMD_CREATE_ARCHIVE,
        &CreateArchiveRequest {
            working_directory: working_directory.to_string(),
            target: target.to_string(),
            output_path: output_path.to_string(),
        },
    )
}

pub fn extract_archive(
    addr: &str,
    working_directory: &str,
    archive_path: &str,
) -> anyhow::Result<GenericResponse> {
    send_unary(
        addr,
        SYNC_CMD_EXTRACT_ARCHIVE,
        &ExtractArchiveRequest {
            working_directory: working_directory.to_string(),
            archive_path: archive_path.to_string(),
        },
    )
}

pub fn chmod(
    addr: &str,
    path: &str,
    mode: &str,
    recursive: bool,
) -> anyhow::Result<GenericResponse> {
    send_unary(
        addr,
        SYNC_CMD_CHMOD,
        &ChmodRequest {
            path: path.to_string(),
            mode: mode.to_string(),
            recursive,
        },
    )
}

pub fn chown(
    addr: &str,
    path: &str,
    uid: i32,
    gid: i32,
    recursive: bool,
) -> anyhow::Result<GenericResponse> {
    send_unary(
        addr,
        SYNC_CMD_CHOWN,
        &ChownRequest {
            path: path.to_string(),
            uid,
            gid,
            recursive,
        },
    )
}

pub fn get_file_owner(addr: &str, path: &str) -> anyhow::Result<i32> {
    let response: GetFileOwnerResponse = send_unary(
        addr,
        SYNC_CMD_GET_FILE_OWNER,
        &GetFileOwnerRequest {
            path: path.to_string(),
        },
    )?;
    Ok(response.uid)
}

pub fn discover_ftrace_events(addr: &str) -> anyhow::Result<DiscoverFtraceResponse> {
    send_unary(addr, SYNC_CMD_DISCOVER_FTRACE_EVENTS, &Empty {})
}

pub fn discover_pmu_events(addr: &str) -> anyhow::Result<DiscoverPmuResponse> {
    send_unary(addr, SYNC_CMD_DISCOVER_PMU_EVENTS, &Empty {})
}

pub fn get_pmu_hw_counters(addr: &str) -> anyhow::Result<crate::proto::PmuHwCounterResponse> {
    send_unary(addr, SYNC_CMD_GET_PMU_HW_COUNTERS, &Empty {})
}

pub fn discover_gpu_counters(addr: &str) -> anyhow::Result<GcDiscoverResponse> {
    send_unary(addr, SYNC_CMD_DISCOVER_GPU_COUNTERS, &GcDiscoverRequest {})
}

pub fn screenshot<F>(
    addr: &str,
    quality: i32,
    local_path: &str,
    progress_cb: F,
) -> anyhow::Result<()>
where
    F: Fn(u64, u64),
{
    receive_file_stream(
        addr,
        SYNC_CMD_SCREENSHOT,
        &ScreenshotRequest { quality },
        local_path,
        progress_cb,
    )
}

pub fn pull_file<F>(
    addr: &str,
    remote_path: &str,
    local_path: &str,
    progress_cb: F,
) -> anyhow::Result<()>
where
    F: Fn(u64, u64),
{
    receive_file_stream(
        addr,
        SYNC_CMD_PULL_FILE,
        &PullFileRequest {
            remote_path: remote_path.to_string(),
        },
        local_path,
        progress_cb,
    )
}

pub fn push_file<F>(
    addr: &str,
    local_path: &str,
    remote_path: &str,
    progress_cb: F,
) -> anyhow::Result<u64>
where
    F: Fn(u64, u64),
{
    let socket_addr = resolve_socket_addr(addr)?;
    let mut stream = TcpStream::connect_timeout(&socket_addr, Duration::from_secs(2))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    stream.write_all(&[SYNC_CMD_PUSH_FILE])?;

    let mut file = File::open(local_path)?;
    let total = file.metadata().map(|m| m.len()).unwrap_or(0);
    let mut sent = 0u64;
    let mut buf = vec![0u8; 64 * 1024];

    if total == 0 {
        write_message(
            &mut stream,
            &FileUploadChunk {
                remote_path: remote_path.to_string(),
                data: Vec::new(),
                is_last: true,
            },
        )?;
    } else {
        loop {
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            sent += n as u64;
            write_message(
                &mut stream,
                &FileUploadChunk {
                    remote_path: remote_path.to_string(),
                    data: buf[..n].to_vec(),
                    is_last: sent >= total,
                },
            )?;
            progress_cb(sent, total);
            if sent >= total {
                break;
            }
        }
    }

    stream.flush()?;
    read_status(&mut stream)?;
    let response: PushFileResponse = read_message(&mut stream)?;
    if !response.success {
        anyhow::bail!("sync push failed after {} bytes", response.bytes_written);
    }
    progress_cb(response.bytes_written, response.bytes_written);
    Ok(response.bytes_written)
}

pub fn start_perfetto(
    addr: &str,
    pid: i32,
    mode: &str,
    duration_secs: i32,
    config_pbtxt: &str,
) -> anyhow::Result<PerfettoResponse> {
    send_unary(
        addr,
        SYNC_CMD_START_PERFETTO,
        &PerfettoRequest {
            pid,
            mode: mode.to_string(),
            duration_secs,
            config_pbtxt: config_pbtxt.to_string(),
        },
    )
}

pub fn get_perfetto_status(addr: &str) -> anyhow::Result<PerfettoStatus> {
    send_unary(addr, SYNC_CMD_GET_PERFETTO_STATUS, &Empty {})
}

fn send_unary<Req, Resp>(addr: &str, opcode: u8, request: &Req) -> anyhow::Result<Resp>
where
    Req: Message,
    Resp: Message + Default,
{
    let socket_addr = resolve_socket_addr(addr)?;
    let mut stream = TcpStream::connect_timeout(&socket_addr, Duration::from_secs(2))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;

    stream.write_all(&[opcode])?;
    write_message(&mut stream, request)?;
    stream.flush()?;

    read_status(&mut stream)?;
    read_message(&mut stream)
}

fn receive_file_stream<Req, F>(
    addr: &str,
    opcode: u8,
    request: &Req,
    local_path: &str,
    progress_cb: F,
) -> anyhow::Result<()>
where
    Req: Message,
    F: Fn(u64, u64),
{
    let socket_addr = resolve_socket_addr(addr)?;
    let mut stream = TcpStream::connect_timeout(&socket_addr, Duration::from_secs(2))?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(30)))?;
    stream.set_write_timeout(Some(Duration::from_secs(30)))?;
    stream.write_all(&[opcode])?;
    write_message(&mut stream, request)?;
    stream.flush()?;

    read_status(&mut stream)?;

    let mut file = File::create(local_path)?;
    let mut received = 0u64;
    loop {
        let chunk: FileChunk = read_message(&mut stream)?;
        if !chunk.data.is_empty() {
            file.write_all(&chunk.data)?;
            received += chunk.data.len() as u64;
            progress_cb(received, 0);
        }
        if chunk.is_last {
            break;
        }
    }
    file.flush()?;
    progress_cb(received, received);
    Ok(())
}

fn write_message<T>(stream: &mut TcpStream, message: &T) -> anyhow::Result<()>
where
    T: Message,
{
    let bytes = message.encode_to_vec();
    stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
    stream.write_all(&bytes)?;
    Ok(())
}

fn resolve_socket_addr(addr: &str) -> anyhow::Result<SocketAddr> {
    addr.to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow::anyhow!("invalid sync control address: {addr}"))
}

fn read_status(stream: &mut TcpStream) -> anyhow::Result<()> {
    let mut status = [0u8; 1];
    stream.read_exact(&mut status)?;
    if status[0] == 0 {
        return Ok(());
    }

    let message = read_error_message(stream)?;
    anyhow::bail!("sync command rejected: {message}")
}

fn read_error_message(stream: &mut TcpStream) -> anyhow::Result<String> {
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 64 * 1024 {
        anyhow::bail!("sync error message too large: {len}");
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    Ok(String::from_utf8_lossy(&buf).into_owned())
}

fn read_message<T>(stream: &mut TcpStream) -> anyhow::Result<T>
where
    T: Message + Default,
{
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > 1024 * 1024 {
        anyhow::bail!("sync response too large: {len}");
    }
    let mut buf = vec![0u8; len];
    stream.read_exact(&mut buf)?;
    Ok(T::decode(&buf[..])?)
}
