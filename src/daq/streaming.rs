use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crossbeam_queue::ArrayQueue;
use log::{debug, error, info, warn};

use super::config::{DaqConfig, Rgb, build_save_name, power_pair_for_channel};
use super::constants;
use super::device::DeviceInfo;
use super::ffi_daqmx::DaqmxLib;
use super::reader::{AccumulationBuffer, register_callback};
use super::scale;
use super::stats;
use super::task::DaqTask;

/// A single poll result pushed to the queue
#[derive(Debug, Clone)]
pub struct DaqPollData {
    pub timestamp_ms: u64,
    /// (pair_name, power_mW)
    pub power_pairs: Vec<(String, f64)>,
    /// (channel_name, mean_value)
    pub channel_values: Vec<(String, f64)>,
}

/// Per-channel metadata needed for the streaming session
#[derive(Debug, Clone)]
pub struct ChannelMeta {
    pub name: String,
    pub color: Rgb,
    /// (pair_index, is_current)
    pub power_pair: Option<(u32, bool)>,
}

/// Summary statistics for a single channel
#[derive(Debug, Clone)]
pub struct DaqChannelSummary {
    pub name: String,
    pub mean: f64,
    pub min: f64,
    pub max: f64,
    pub rms: f64,
    pub color_r: u8,
    pub color_g: u8,
    pub color_b: u8,
    pub is_current: bool,
    pub pair_index: i32,
}

/// Power pair summary
#[derive(Debug, Clone)]
pub struct DaqPowerPairSummary {
    pub name: String,
    pub avg_power_mw: f64,
}

/// Summary returned when stopping
#[derive(Debug, Clone)]
pub struct DaqSummary {
    pub channels: Vec<DaqChannelSummary>,
    pub power_breakdown: Vec<DaqPowerPairSummary>,
    pub total_power_mw: f64,
    pub measurement_time_s: f64,
}

/// Handle for a running DAQ streaming session
pub struct DaqStreamHandle {
    cancelled: Arc<AtomicBool>,
    queue: Arc<ArrayQueue<DaqPollData>>,
    error_queue: Arc<ArrayQueue<String>>,
    /// Background thread doing the polling loop
    thread_handle: Option<std::thread::JoinHandle<DaqSessionResult>>,
}

/// Result carried by the background thread
struct DaqSessionResult {
    all_samples: Vec<Vec<f64>>,
    channel_metas: Vec<ChannelMeta>,
    #[allow(dead_code)]
    sample_rate: f64,
    start_time: Instant,
}

impl DaqStreamHandle {
    pub fn start(
        config: &DaqConfig,
        sample_rate: u32,
        terminal_diff: bool,
        max_voltage: f64,
        min_voltage: f64,
    ) -> Result<Self, String> {
        info!("DAQ: Loading NI-DAQmx library...");
        let lib = DaqmxLib::load().map_err(|e| {
            error!("DAQ: Failed to load NI-DAQmx library: {}", e);
            e.to_string()
        })?;
        info!("DAQ: NI-DAQmx library loaded successfully");

        info!("DAQ: Enumerating devices...");
        let devices = super::device::enumerate_devices(&lib).map_err(|e| {
            error!("DAQ: Device enumeration failed: {}", e);
            e.to_string()
        })?;

        if devices.is_empty() {
            error!("DAQ: No DAQmx devices found");
            return Err("No DAQmx devices found".to_string());
        }
        info!("DAQ: Found {} device(s)", devices.len());

        let terminal_config = if terminal_diff {
            constants::DAQmx_Val_Diff
        } else {
            constants::DAQmx_Val_RSE
        };

        // Build channel list per device, create tasks, register callbacks
        let mut all_channel_metas: Vec<ChannelMeta> = Vec::new();
        let mut all_buffers: Vec<Arc<Mutex<AccumulationBuffer>>> = Vec::new();
        // We need to keep tasks and callback contexts alive
        let mut task_handles: Vec<constants::TaskHandle> = Vec::new();

        // We store raw pointers to keep DaqmxLib alive in the thread
        let lib = Arc::new(lib);

        for dev in &devices {
            let dev_channels = channels_for_device(config, dev);
            if dev_channels.is_empty() {
                continue;
            }

            let lib_ref: &DaqmxLib = &lib;

            // Create scales
            let mut scale_names: Vec<String> = Vec::new();
            for ch in &dev_channels {
                let pp = power_pair_for_channel(config, ch);
                let save_name = build_save_name(&ch.physical_channel, &ch.name, &ch.color, pp);
                scale::create_linear_scale(lib_ref, &save_name, ch.gain, ch.offset)
                    .map_err(|e| {
                        error!("DAQ: Failed to create scale for channel '{}': {}", ch.name, e);
                        format!("Failed to create scale for {}: {}", ch.name, e)
                    })?;
                scale_names.push(save_name);
            }

            // Create task
            let task_name = format!("mprofiler_{}", dev.name);
            debug!("DAQ: Creating task '{}' for device '{}'", task_name, dev.name);
            let task = DaqTask::new(lib_ref, &task_name)
                .map_err(|e| {
                    error!("DAQ: Failed to create task for device '{}': {}", dev.name, e);
                    format!("Failed to create task for {}: {}", dev.name, e)
                })?;

            // Add channels
            let mut num_valid = 0;
            for (idx, ch) in dev_channels.iter().enumerate() {
                let ch_num = ch
                    .physical_channel
                    .rsplit("ai")
                    .next()
                    .and_then(|s| s.parse::<u32>().ok())
                    .unwrap_or(0);
                if ch_num % 16 >= 8 && terminal_config == constants::DAQmx_Val_Diff {
                    continue;
                }

                let ch_min = ch.range_min.unwrap_or(min_voltage);
                let ch_max = ch.range_max.unwrap_or(max_voltage);
                let scaled_min = ch_min * ch.gain + ch.offset;
                let scaled_max = ch_max * ch.gain + ch.offset;

                task.add_ai_voltage_chan(
                    &ch.physical_channel,
                    &scale_names[idx],
                    terminal_config,
                    scaled_min,
                    scaled_max,
                    true,
                    &scale_names[idx],
                )
                .map_err(|e| {
                    error!("DAQ: Failed to add channel '{}': {}", ch.physical_channel, e);
                    format!("Failed to add channel {}: {}", ch.physical_channel, e)
                })?;

                let pp = power_pair_for_channel(config, ch);
                all_channel_metas.push(ChannelMeta {
                    name: ch.name.clone(),
                    color: ch.color.clone(),
                    power_pair: pp,
                });
                num_valid += 1;
            }

            if num_valid == 0 {
                continue;
            }

            // Configure timing (continuous)
            task.cfg_timing(
                sample_rate as f64,
                constants::DAQmx_Val_ContSamps,
                sample_rate as u64, // buffer size = 1 second
            )
            .map_err(|e| {
                error!("DAQ: Failed to configure timing for device '{}': {}", dev.name, e);
                format!("Failed to configure timing: {}", e)
            })?;

            // Configure input buffer: 2 seconds to avoid overflow at high sample rates
            let buf_size = sample_rate * 2;
            task.cfg_input_buffer(buf_size).map_err(|e| {
                error!("DAQ: Failed to configure input buffer for device '{}': {}", dev.name, e);
                format!("Failed to configure input buffer: {}", e)
            })?;

            // Callback interval: ~100ms worth of samples
            let interval = (sample_rate / 10).max(1);
            let buffer = Arc::new(Mutex::new(AccumulationBuffer::new(num_valid)));
            all_buffers.push(buffer.clone());

            let ctx = register_callback(&task, num_valid, interval, buffer)
                .map_err(|e| {
                    error!("DAQ: Failed to register callback for device '{}': {}", dev.name, e);
                    format!("Failed to register callback: {}", e)
                })?;

            task.start()
                .map_err(|e| {
                    error!("DAQ: Failed to start task for device '{}': {}", dev.name, e);
                    format!("Failed to start task: {}", e)
                })?;
            info!("DAQ: Task '{}' started with {} channels, interval={} samples", task_name, num_valid, interval);

            let handle = task.handle;
            task_handles.push(handle);
            // Prevent Drop from clearing the task — we'll do it manually
            std::mem::forget(task);
            std::mem::forget(ctx);
        }

        if task_handles.is_empty() {
            error!("DAQ: No valid channels found for any device");
            return Err("No valid channels found for any device".to_string());
        }

        let cancelled = Arc::new(AtomicBool::new(false));
        let queue = Arc::new(ArrayQueue::new(256));
        let error_queue = Arc::new(ArrayQueue::new(64));

        let cancelled_clone = cancelled.clone();
        let queue_clone = queue.clone();
        let error_queue_clone = error_queue.clone();
        let channel_metas_clone = all_channel_metas.clone();
        let rate = sample_rate as f64;
        let lib_clone = lib.clone();

        info!("DAQ: Starting background streaming thread with {} channels", all_channel_metas.len());

        // Background thread: periodically drain buffers, compute power, push to queue
        let thread_handle = std::thread::spawn(move || {
            let start_time = Instant::now();
            // Accumulate all samples for final summary
            let total_channels: usize = all_channel_metas.len();
            let mut cumulative_samples: Vec<Vec<f64>> = (0..total_channels).map(|_| Vec::new()).collect();

            // Compute how many samples correspond to the poll interval (~500ms)
            let poll_interval = std::time::Duration::from_millis(500);
            let mut poll_count: u64 = 0;
            let mut total_samples_collected: u64 = 0;

            while !cancelled_clone.load(Ordering::Relaxed) {
                std::thread::sleep(poll_interval);

                // Drain all accumulated data from all buffers
                let mut channel_offset = 0;
                let mut chunk_data: Vec<Vec<f64>> = (0..total_channels).map(|_| Vec::new()).collect();

                for (buf_idx, buf) in all_buffers.iter().enumerate() {
                    match buf.lock() {
                        Ok(mut locked) => {
                            let taken = locked.take();
                            for (i, ch_data) in taken.into_iter().enumerate() {
                                if channel_offset + i < total_channels {
                                    chunk_data[channel_offset + i] = ch_data;
                                }
                            }
                            channel_offset += locked.data.len();
                        }
                        Err(e) => {
                            let msg = format!("Buffer mutex poisoned for buffer {}: {}", buf_idx, e);
                            error!("DAQ: {}", msg);
                            let _ = error_queue_clone.push(msg);
                        }
                    }
                }

                // Check if we got any data
                let has_data = chunk_data.iter().any(|ch| !ch.is_empty());
                if !has_data {
                    continue;
                }

                // Accumulate for summary
                let chunk_sample_count: usize = chunk_data.iter().map(|ch| ch.len()).max().unwrap_or(0);
                total_samples_collected += chunk_sample_count as u64;
                for (i, ch) in chunk_data.iter().enumerate() {
                    cumulative_samples[i].extend_from_slice(ch);
                }
                poll_count += 1;

                let elapsed_ms = start_time.elapsed().as_millis() as u64;

                // Compute per-channel means for this chunk
                let mut channel_values: Vec<(String, f64)> = Vec::new();
                for (i, ch_data) in chunk_data.iter().enumerate() {
                    let mean = if ch_data.is_empty() {
                        0.0
                    } else {
                        ch_data.iter().sum::<f64>() / ch_data.len() as f64
                    };
                    channel_values.push((channel_metas_clone[i].name.clone(), mean));
                }

                // Compute per-power-pair power
                let mut power_pairs: Vec<(String, f64)> = Vec::new();
                let mut pair_indices: std::collections::HashSet<u32> = std::collections::HashSet::new();
                for meta in &channel_metas_clone {
                    if let Some((idx, _)) = meta.power_pair {
                        pair_indices.insert(idx);
                    }
                }
                let mut pair_indices: Vec<u32> = pair_indices.into_iter().collect();
                pair_indices.sort();

                for pair_idx in &pair_indices {
                    // Find current and voltage channels for this pair
                    let mut current_data: Option<&Vec<f64>> = None;
                    let mut voltage_data: Option<&Vec<f64>> = None;
                    let mut current_name = String::new();
                    let mut voltage_name = String::new();

                    for (i, meta) in channel_metas_clone.iter().enumerate() {
                        if let Some((idx, is_current)) = meta.power_pair {
                            if idx == *pair_idx {
                                if is_current {
                                    current_data = Some(&chunk_data[i]);
                                    current_name = meta.name.clone();
                                } else {
                                    voltage_data = Some(&chunk_data[i]);
                                    voltage_name = meta.name.clone();
                                }
                            }
                        }
                    }

                    if let (Some(v_data), Some(i_data)) = (voltage_data, current_data) {
                        // V * I = power (already in scaled units from gain/offset)
                        // Result is in watts if V is volts and I is amps
                        // Convert to mW
                        let power_w = stats::compute_power(v_data, i_data);
                        let power_mw = power_w * 1000.0;

                        // Use voltage channel name as pair name (remove I/V suffix)
                        let pair_name = if !voltage_name.is_empty() {
                            voltage_name
                        } else {
                            current_name
                        };
                        power_pairs.push((pair_name, power_mw));
                    }
                }

                let poll_data = DaqPollData {
                    timestamp_ms: elapsed_ms,
                    power_pairs,
                    channel_values,
                };

                // Non-blocking push; if queue is full, drop oldest
                if queue_clone.push(poll_data).is_err() {
                    warn!("DAQ: Poll queue full, dropping data point");
                }

                if poll_count % 20 == 0 {
                    debug!("DAQ: Poll #{}, total samples collected: {}, elapsed: {:.1}s",
                        poll_count, total_samples_collected, start_time.elapsed().as_secs_f64());
                }
            }

            info!("DAQ: Streaming stopped after {:.1}s, {} polls, {} total samples",
                start_time.elapsed().as_secs_f64(), poll_count, total_samples_collected);

            // Clean up: stop and clear all tasks
            for (i, &handle) in task_handles.iter().enumerate() {
                unsafe {
                    let stop_code = (lib_clone.stop_task)(handle);
                    if stop_code != 0 {
                        let msg = format!("stop_task[{}] returned error code {}", i, stop_code);
                        warn!("DAQ: {}", msg);
                        let _ = error_queue_clone.push(msg);
                    }
                    let clear_code = (lib_clone.clear_task)(handle);
                    if clear_code != 0 {
                        let msg = format!("clear_task[{}] returned error code {}", i, clear_code);
                        warn!("DAQ: {}", msg);
                        let _ = error_queue_clone.push(msg);
                    }
                }
            }
            info!("DAQ: All tasks cleaned up");

            DaqSessionResult {
                all_samples: cumulative_samples,
                channel_metas: channel_metas_clone,
                sample_rate: rate,
                start_time,
            }
        });

        Ok(Self {
            cancelled,
            queue,
            error_queue,
            thread_handle: Some(thread_handle),
        })
    }

    /// Non-blocking poll for next data point
    pub fn poll(&self) -> Option<DaqPollData> {
        self.queue.pop()
    }

    /// Drain all pending error messages from the streaming thread
    pub fn drain_errors(&self) -> Vec<String> {
        let mut errors = Vec::new();
        while let Some(e) = self.error_queue.pop() {
            errors.push(e);
        }
        errors
    }

    /// Stop the streaming session and return summary statistics
    pub fn stop(mut self) -> Result<DaqSummary, String> {
        info!("DAQ: Stopping streaming session...");
        self.cancelled.store(true, Ordering::Relaxed);

        let result = if let Some(handle) = self.thread_handle.take() {
            match handle.join() {
                Ok(r) => r,
                Err(panic_info) => {
                    let panic_msg = if let Some(s) = panic_info.downcast_ref::<String>() {
                        s.clone()
                    } else if let Some(s) = panic_info.downcast_ref::<&str>() {
                        s.to_string()
                    } else {
                        "unknown panic".to_string()
                    };
                    error!("DAQ: Background thread panicked: {}", panic_msg);
                    return Err(format!("DAQ background thread panicked: {}", panic_msg));
                }
            }
        } else {
            warn!("DAQ: No thread handle available (already stopped?)");
            return Err("DAQ: No thread handle available (already stopped?)".to_string());
        };

        let measurement_time_s = result.start_time.elapsed().as_secs_f64();

        // Compute per-channel stats
        let mut channel_summaries = Vec::new();
        for (i, meta) in result.channel_metas.iter().enumerate() {
            let s = stats::compute_stats(&result.all_samples[i]);
            let (is_current, pair_index) = match meta.power_pair {
                Some((idx, is_c)) => (is_c, idx as i32),
                None => (false, -1),
            };
            channel_summaries.push(DaqChannelSummary {
                name: meta.name.clone(),
                mean: s.mean,
                min: s.min,
                max: s.max,
                rms: s.rms,
                color_r: meta.color.r,
                color_g: meta.color.g,
                color_b: meta.color.b,
                is_current,
                pair_index,
            });
        }

        // Compute power breakdown
        let mut pair_indices: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
        for meta in &result.channel_metas {
            if let Some((idx, _)) = meta.power_pair {
                pair_indices.insert(idx);
            }
        }

        let mut power_breakdown = Vec::new();
        let mut total_power_mw = 0.0;

        for pair_idx in &pair_indices {
            let mut current_samples: Option<&Vec<f64>> = None;
            let mut voltage_samples: Option<&Vec<f64>> = None;
            let mut voltage_name = String::new();
            let mut current_name = String::new();

            for (i, meta) in result.channel_metas.iter().enumerate() {
                if let Some((idx, is_current)) = meta.power_pair {
                    if idx == *pair_idx {
                        if is_current {
                            current_samples = Some(&result.all_samples[i]);
                            current_name = meta.name.clone();
                        } else {
                            voltage_samples = Some(&result.all_samples[i]);
                            voltage_name = meta.name.clone();
                        }
                    }
                }
            }

            if let (Some(v), Some(i)) = (voltage_samples, current_samples) {
                let power_w = stats::compute_power(v, i);
                let power_mw = power_w * 1000.0;
                total_power_mw += power_mw;

                let pair_name = if !voltage_name.is_empty() {
                    voltage_name
                } else {
                    current_name
                };
                power_breakdown.push(DaqPowerPairSummary {
                    name: pair_name,
                    avg_power_mw: power_mw,
                });
            }
        }

        info!("DAQ: Summary computed — {} channels, {} power pairs, total {:.1} mW, {:.1}s",
            channel_summaries.len(), power_breakdown.len(), total_power_mw, measurement_time_s);

        Ok(DaqSummary {
            channels: channel_summaries,
            power_breakdown,
            total_power_mw,
            measurement_time_s,
        })
    }

    /// Cancel without waiting for join (used in shutdown)
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

fn channels_for_device<'a>(config: &'a DaqConfig, device: &DeviceInfo) -> Vec<&'a super::config::ChannelConfig> {
    config
        .channels
        .iter()
        .filter(|ch| {
            ch.physical_channel.starts_with(&device.name)
                || device.ai_channels.contains(&ch.physical_channel)
        })
        .collect()
}
