//! Calibration storage: exact hardware-indexed 100-ms channel statistics.
//! Acquire every ADC sample, but retain means/counts/extrema/second moments.
use flate2::{write::GzEncoder, Compression};
use std::{
    collections::VecDeque,
    fs::File,
    io::{self, Write},
    path::PathBuf,
};

pub const MAX_DAQ_BYTES: u64 = 7_000_000_000;
#[derive(Clone)]
struct BinStats {
    n: usize,
    sum: f64,
    sq: f64,
    min: f64,
    max: f64,
}
impl BinStats {
    fn empty() -> Self {
        Self {
            n: 0,
            sum: 0.,
            sq: 0.,
            min: f64::INFINITY,
            max: f64::NEG_INFINITY,
        }
    }
    fn add(&mut self, v: f64) {
        self.n += 1;
        self.sum += v;
        self.sq += v * v;
        self.min = self.min.min(v);
        self.max = self.max.max(v);
    }
}
pub struct CompactWriter {
    file: File,
    bytes: u64,
    limit: u64,
    rate: u32,
    bin: usize,
    queues: Vec<VecDeque<BinStats>>,
    partial: Vec<BinStats>,
    index: u64,
    pending: Vec<u8>,
    pending_rows: usize,
    committed_path: Option<PathBuf>,
}
fn name(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
impl CompactWriter {
    pub fn new(path: &str, names: &[String], rate: u32) -> io::Result<Self> {
        let mut writer = Self::with_limit(File::create(path)?, names, rate, MAX_DAQ_BYTES)?;
        writer.committed_path = Some(PathBuf::from(format!("{path}.committed")));
        writer.publish_commit()?;
        Ok(writer)
    }
    fn with_limit(file: File, names: &[String], rate: u32, limit: u64) -> io::Result<Self> {
        if rate < 10 || rate % 10 != 0 || names.is_empty() {
            return Err(io::Error::other(
                "Compact calibration requires sample rate divisible by 10 and channels",
            ));
        }
        let mut pending = b"timestamp_s".to_vec();
        for prefix in ["", "__n:", "__min:", "__max:", "__msq:"] {
            for n in names {
                write!(pending, ",{}", name(&format!("{prefix}{n}")))?;
            }
        }
        pending.push(b'\n');
        let mut writer = Self {
            file,
            bytes: 0,
            limit,
            rate,
            bin: (rate / 10) as usize,
            queues: vec![VecDeque::new(); names.len()],
            partial: vec![BinStats::empty(); names.len()],
            index: 0,
            pending,
            pending_rows: 0,
            committed_path: None,
        };
        writer.flush_member()?;
        Ok(writer)
    }
    pub fn push(&mut self, channels: &[Vec<f64>]) -> io::Result<()> {
        if channels.len() != self.queues.len() {
            return Err(io::Error::other("Compact channel count changed"));
        }
        for ((queue, partial), channel) in
            self.queues.iter_mut().zip(&mut self.partial).zip(channels)
        {
            if channel.iter().any(|v| !v.is_finite()) {
                return Err(io::Error::other("Nonfinite ADC sample"));
            }
            for &v in channel {
                partial.add(v);
                if partial.n == self.bin {
                    queue.push_back(std::mem::replace(partial, BinStats::empty()));
                }
            }
            // Independent NI task clocks can accumulate seconds of drift over
            // 72h. Queue bounded statistics, never that many raw ADC samples.
            if queue.len() > 3000 {
                return Err(io::Error::other(
                    "DAQ devices diverged by over 300 seconds; continuity unknown",
                ));
            }
        }
        while self.queues.iter().all(|q| !q.is_empty()) {
            self.emit(false)?;
        }
        Ok(())
    }
    fn emit(&mut self, partial: bool) -> io::Result<()> {
        let mut stats = Vec::with_capacity(self.queues.len());
        for (queue, tail) in self.queues.iter_mut().zip(&mut self.partial) {
            let s = queue.pop_front().unwrap_or_else(|| {
                if partial {
                    std::mem::replace(tail, BinStats::empty())
                } else {
                    BinStats::empty()
                }
            });
            stats.push((
                s.n,
                s.sum / s.n.max(1) as f64,
                s.min,
                s.max,
                s.sq / s.n.max(1) as f64,
            ));
        }
        write!(self.pending, "{:.6}", self.index as f64 / self.rate as f64)?;
        for field in 0..5 {
            for &(n, mean, min, max, msq) in &stats {
                if field == 1 {
                    write!(self.pending, ",{n}")?;
                } else if n == 0 {
                    write!(self.pending, ",")?;
                } else {
                    write!(self.pending, ",{:.9}", [mean, 0.0, min, max, msq][field])?;
                }
            }
        }
        self.pending.push(b'\n');
        self.index += self.bin as u64;
        self.pending_rows += 1;
        if self.pending_rows >= 10 {
            self.flush_member()?;
        }
        Ok(())
    }
    fn flush_member(&mut self) -> io::Result<()> {
        if self.pending.is_empty() {
            return Ok(());
        }
        let mut encoder = GzEncoder::new(Vec::new(), Compression::new(6));
        encoder.write_all(&self.pending)?;
        let member = encoder.finish()?;
        if self.bytes + member.len() as u64 > self.limit {
            return Err(io::Error::other(
                "DAQ compressed storage cap reached; completed members retained",
            ));
        }
        self.file.write_all(&member)?;
        self.file.flush()?;
        self.bytes += member.len() as u64;
        self.pending.clear();
        self.pending_rows = 0;
        self.publish_commit()
    }
    fn publish_commit(&self) -> io::Result<()> {
        if let Some(path) = &self.committed_path {
            let temporary = PathBuf::from(format!("{}.tmp", path.display()));
            std::fs::write(&temporary, self.bytes.to_string())?;
            std::fs::rename(temporary, path)?;
        }
        Ok(())
    }
    pub fn finish(&mut self) -> io::Result<()> {
        while self.queues.iter().any(|q| !q.is_empty()) || self.partial.iter().any(|p| p.n > 0) {
            self.emit(true)?;
        }
        self.flush_member()?;
        self.file.sync_data()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    #[test]
    fn preserves_all_samples_across_unequal_callback_boundaries_and_partial_tail() {
        let p = std::env::temp_dir().join(format!("daq-compact-{}.gz", std::process::id()));
        let mut w =
            CompactWriter::new(p.to_str().unwrap(), &["A".into(), "B".into()], 1000).unwrap();
        w.push(&[vec![1.0; 60], vec![2.0; 100]]).unwrap();
        w.push(&[vec![3.0; 55], vec![4.0; 15]]).unwrap();
        w.finish().unwrap();
        let mut text = String::new();
        flate2::read::MultiGzDecoder::new(File::open(&p).unwrap())
            .read_to_string(&mut text)
            .unwrap();
        let rows: Vec<_> = text
            .lines()
            .skip(1)
            .map(|l| l.split(',').collect::<Vec<_>>())
            .collect();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            &rows[0][1..5],
            &["1.800000000", "2.000000000", "100", "100"]
        );
        assert_eq!(&rows[1][1..5], &["3.000000000", "4.000000000", "15", "15"]);
        assert_eq!(rows[0][9], "4.200000000");
        std::fs::remove_file(p).unwrap();
    }
    #[test]
    fn cap_rejects_member_before_writing_and_keeps_a_valid_gzip_prefix() {
        let p = std::env::temp_dir().join(format!("daq-cap-{}.gz", std::process::id()));
        let mut w =
            CompactWriter::with_limit(File::create(&p).unwrap(), &["A".into()], 1000, 300).unwrap();
        w.limit = w.bytes;
        assert!(w.push(&[vec![1.; 1000]]).is_err());
        let mut text = String::new();
        flate2::read::MultiGzDecoder::new(File::open(&p).unwrap())
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text.lines().count(), 1);
        std::fs::remove_file(p).unwrap();
    }
    #[test]
    fn independent_task_drift_is_buffered_as_statistics_without_raw_sample_backlog() {
        let p = std::env::temp_dir().join(format!("daq-drift-{}.gz", std::process::id()));
        let mut w =
            CompactWriter::new(p.to_str().unwrap(), &["A".into(), "B".into()], 1000).unwrap();
        w.push(&[vec![1.; 150_000], vec![2.; 100_000]]).unwrap();
        assert_eq!(w.queues[0].len(), 500); // 50 seconds of stats, not 50,000 raw samples
        assert_eq!(w.partial[0].n, 0);
        w.push(&[vec![], vec![2.; 50_000]]).unwrap();
        w.finish().unwrap();
        let mut text = String::new();
        flate2::read::MultiGzDecoder::new(File::open(&p).unwrap())
            .read_to_string(&mut text)
            .unwrap();
        assert_eq!(text.lines().count(), 1501);
        assert!(text.lines().skip(1).all(|line| line
            .split(',')
            .skip(3)
            .take(2)
            .collect::<Vec<_>>()
            == ["100", "100"]));
        std::fs::remove_file(p).unwrap();
    }
}
