use anyhow::{Context, Result};
use log::{debug, info};
use std::path::Path;

/// RGB color
#[derive(Debug, Clone, Default)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    pub fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

/// Single channel configuration from Excel
#[derive(Debug, Clone)]
pub struct ChannelConfig {
    pub physical_channel: String,
    pub name: String,
    pub gain: f64,
    pub offset: f64,
    pub order: Option<u32>,
    pub range_max: Option<f64>,
    pub range_min: Option<f64>,
    pub color: Rgb,
}

/// Power pair configuration
#[derive(Debug, Clone)]
pub struct PowerPair {
    pub index: u32,
    pub current_channel: String,
    pub voltage_channel: String,
}

/// Full DAQ configuration loaded from Excel
#[derive(Debug, Clone)]
pub struct DaqConfig {
    pub channels: Vec<ChannelConfig>,
    pub power_pairs: Vec<PowerPair>,
    pub has_order: bool,
    pub has_range: bool,
}

/// Illegal characters in file names
const WRONG_SYMBOLS: &[char] = &[
    '%', '*', '[', ']', ':', '<', '>', '?', '|', '"', '/', '\\', ',', '.',
];

fn parse_numeric_cell(value: &str) -> Option<f64> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    let expr = if let Some(stripped) = trimmed.strip_prefix('=') {
        stripped.trim()
    } else {
        trimmed
    };
    if let Ok(v) = expr.parse::<f64>() {
        return Some(v);
    }
    if let Some((num_str, den_str)) = expr.split_once('/') {
        if let (Ok(num), Ok(den)) = (num_str.trim().parse::<f64>(), den_str.trim().parse::<f64>())
        {
            if den != 0.0 {
                return Some(num / den);
            }
        }
    }
    if let Some((a_str, b_str)) = expr.split_once('*') {
        if let (Ok(a), Ok(b)) = (a_str.trim().parse::<f64>(), b_str.trim().parse::<f64>()) {
            return Some(a * b);
        }
    }
    None
}

fn extract_cell_color(
    worksheet: &umya_spreadsheet::Worksheet,
    row: u32,
    col: u32,
) -> Rgb {
    let cell = match worksheet.get_cell((col, row)) {
        Some(c) => c,
        None => return Rgb::new(0xFF, 0xFF, 0xFF),
    };

    let style = cell.get_style();
    let fill = match style.get_fill() {
        Some(f) => f,
        None => return Rgb::new(0xFF, 0xFF, 0xFF),
    };

    let color_obj = fill
        .get_pattern_fill()
        .and_then(|pf| pf.get_foreground_color());

    let hex_str = match color_obj {
        Some(c) => c.get_argb().to_string(),
        None => return Rgb::new(0xFF, 0xFF, 0xFF),
    };

    let hex = hex_str.trim_start_matches('#');
    let rgb_hex = if hex.len() == 8 {
        &hex[2..]
    } else if hex.len() == 6 {
        hex
    } else {
        return Rgb::new(0xFF, 0xFF, 0xFF);
    };

    let r = u8::from_str_radix(&rgb_hex[0..2], 16).unwrap_or(0xFF);
    let g = u8::from_str_radix(&rgb_hex[2..4], 16).unwrap_or(0xFF);
    let b = u8::from_str_radix(&rgb_hex[4..6], 16).unwrap_or(0xFF);

    if r == 0 && g == 0 && b == 0 {
        Rgb::new(0xFF, 0xFF, 0xFF)
    } else {
        Rgb::new(r, g, b)
    }
}

fn cell_value(worksheet: &umya_spreadsheet::Worksheet, row: u32, col: u32) -> String {
    worksheet
        .get_cell((col, row))
        .map(|c| c.get_value().to_string())
        .unwrap_or_default()
        .trim()
        .to_string()
}

fn convert_voltage_in_name(name: &str) -> String {
    let mut result = name.to_string();
    let parts: Vec<&str> = name.split('_').collect();
    for part in &parts {
        if part.ends_with('V') || part.ends_with('v') {
            let numeric_part = &part[..part.len() - 1];
            if numeric_part.contains('.') {
                if let Ok(voltage) = numeric_part.parse::<f64>() {
                    let mv = (voltage * 1000.0).round() as i64;
                    let old = *part;
                    let new_str = format!("{}mV", mv);
                    result = result.replace(old, &new_str);
                }
            }
        }
    }
    result
}

fn sanitize_name(name: &str) -> Result<String> {
    let sanitized = name.replace(',', ";").replace('/', "_").replace('\\', "_");
    for ch in sanitized.chars() {
        if WRONG_SYMBOLS.contains(&ch) {
            anyhow::bail!(
                "Channel name '{}' contains illegal character '{}'",
                name,
                ch
            );
        }
    }
    Ok(sanitized)
}

pub fn build_save_name(
    physical_channel: &str,
    name: &str,
    color: &Rgb,
    power_pair: Option<(u32, bool)>,
) -> String {
    let phys = physical_channel.replace('/', "_");
    let mut save = format!(
        "{}_{}-CR{:02X}-CG{:02X}-CB{:02X}",
        phys, name, color.r, color.g, color.b
    );
    if let Some((idx, is_current)) = power_pair {
        if is_current {
            save.push_str(&format!("-PI{}", idx));
        } else {
            save.push_str(&format!("-PV{}", idx));
        }
    }
    save
}

/// Determine power pair info for a channel
pub fn power_pair_for_channel(config: &DaqConfig, ch: &ChannelConfig) -> Option<(u32, bool)> {
    for pp in &config.power_pairs {
        if pp.current_channel == ch.physical_channel || pp.current_channel == ch.name {
            return Some((pp.index, true));
        }
        if pp.voltage_channel == ch.physical_channel || pp.voltage_channel == ch.name {
            return Some((pp.index, false));
        }
    }
    None
}

/// Load a DAQ configuration from an Excel (.xlsx) file.
pub fn load_config(path: &Path) -> Result<DaqConfig> {
    info!("DAQ: Loading config from: {}", path.display());
    let book = umya_spreadsheet::reader::xlsx::read(path)
        .with_context(|| format!("Failed to open Excel file: {}", path.display()))?;

    // --- Sheet 1 (index 1): Power pairs ---
    let mut power_pairs: Vec<PowerPair> = Vec::new();
    if book.get_sheet_count() > 1 {
        let pp_sheet = book
            .get_sheet(&1)
            .with_context(|| "Failed to read power pairs sheet (sheet index 1)")?;
        let (_, max_row) = pp_sheet.get_highest_column_and_row();
        for row in 2..=max_row {
            let index_str = cell_value(pp_sheet, row, 1);
            let current_ch = cell_value(pp_sheet, row, 2);
            let voltage_ch = cell_value(pp_sheet, row, 3);
            if index_str.is_empty() && current_ch.is_empty() {
                continue;
            }
            let index: u32 = index_str
                .parse()
                .with_context(|| format!("Invalid power pair index '{}' at row {}", index_str, row))?;
            power_pairs.push(PowerPair {
                index,
                current_channel: current_ch,
                voltage_channel: voltage_ch,
            });
        }
    }

    // --- Sheet 0: Channel settings ---
    let ch_sheet = book
        .get_sheet(&0)
        .with_context(|| "Failed to read channel settings sheet (sheet index 0)")?;
    let (max_col, max_row) = ch_sheet.get_highest_column_and_row();

    let has_order = max_col >= 5 && {
        let h = cell_value(ch_sheet, 1, 5).to_lowercase();
        h.contains("order")
    };
    let has_range = max_col >= 6 && {
        let h = cell_value(ch_sheet, 1, 6).to_lowercase();
        h.contains("range") || h.contains("max")
    };

    let mut channels: Vec<ChannelConfig> = Vec::new();
    for row in 2..=max_row {
        let physical_channel = cell_value(ch_sheet, row, 1);
        if physical_channel.is_empty() {
            continue;
        }
        let raw_name = cell_value(ch_sheet, row, 2);
        if raw_name.is_empty() {
            continue;
        }
        let gain_str = cell_value(ch_sheet, row, 3);
        let offset_str = cell_value(ch_sheet, row, 4);
        let gain = parse_numeric_cell(&gain_str).unwrap_or(1.0);
        let offset = parse_numeric_cell(&offset_str).unwrap_or(0.0);
        if gain == 0.0 {
            anyhow::bail!(
                "Gain is zero for channel '{}' at row {} -- this would cause division by zero",
                raw_name,
                row
            );
        }
        let order = if has_order {
            let v = cell_value(ch_sheet, row, 5);
            if v.is_empty() {
                None
            } else {
                Some(v.parse::<u32>().with_context(|| {
                    format!("Invalid order '{}' at row {}", v, row)
                })?)
            }
        } else {
            None
        };
        let range_max = if has_range {
            let v = cell_value(ch_sheet, row, 6);
            parse_numeric_cell(&v)
        } else {
            None
        };
        let range_min = if has_range && max_col >= 7 {
            let v = cell_value(ch_sheet, row, 7);
            parse_numeric_cell(&v)
        } else {
            None
        };
        let color = extract_cell_color(ch_sheet, row, 2);
        let name_converted = convert_voltage_in_name(&raw_name);
        let name = sanitize_name(&name_converted)?;

        channels.push(ChannelConfig {
            physical_channel,
            name,
            gain,
            offset,
            order,
            range_max,
            range_min,
            color,
        });
    }

    info!("DAQ: Config loaded — {} channels, {} power pairs (has_order={}, has_range={})",
        channels.len(), power_pairs.len(), has_order, has_range);
    for ch in &channels {
        debug!("DAQ:   Channel '{}' on {} (gain={}, offset={})", ch.name, ch.physical_channel, ch.gain, ch.offset);
    }
    for pp in &power_pairs {
        debug!("DAQ:   PowerPair #{}: I={}, V={}", pp.index, pp.current_channel, pp.voltage_channel);
    }

    Ok(DaqConfig {
        channels,
        power_pairs,
        has_order,
        has_range,
    })
}
