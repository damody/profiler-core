use apache_avro::types::Record;
use apache_avro::{Codec, Schema, Writer};
use log::{error, info};
use std::fs::File;
use std::io::BufWriter;

pub struct DaqAvroWriter {
    writer: Writer<'static, BufWriter<File>>,
    schema: &'static Schema,
    channel_count: usize,
}

impl DaqAvroWriter {
    pub fn new(path: &str, channel_names: &[String]) -> Result<Self, String> {
        let schema = build_schema(channel_names)?;
        // Leak to get 'static lifetime — one allocation per session, acceptable.
        let schema: &'static Schema = Box::leak(Box::new(schema));

        let file = File::create(path).map_err(|e| format!("Avro create failed: {}", e))?;
        let buf = BufWriter::new(file);
        let writer = Writer::with_codec(schema, buf, Codec::Deflate);

        info!("DAQ: Avro streaming to {}", path);
        Ok(Self {
            writer,
            schema,
            channel_count: channel_names.len(),
        })
    }

    /// Write a chunk of samples. `chunk[ch][sample]` layout.
    /// `sample_index` is updated in place (caller tracks it).
    pub fn write_chunk(
        &mut self,
        chunk: &[Vec<f64>],
        sample_index: &mut u64,
        dt: f64,
    ) -> Result<(), String> {
        let sample_count = chunk.iter().map(|ch| ch.len()).max().unwrap_or(0);

        for s in 0..sample_count {
            let mut record = Record::new(self.schema)
                .ok_or_else(|| "Failed to create Avro record".to_string())?;

            record.put("timestamp_s", (*sample_index as f64) * dt);

            for ch_idx in 0..self.channel_count {
                let field_name = format!("ch_{}", ch_idx);
                let value = if ch_idx < chunk.len() && s < chunk[ch_idx].len() {
                    chunk[ch_idx][s]
                } else {
                    0.0
                };
                record.put(&field_name, value);
            }

            self.writer
                .append(record)
                .map_err(|e| format!("Avro append failed: {}", e))?;

            *sample_index += 1;
        }

        Ok(())
    }

    pub fn finish(self) -> Result<(), String> {
        self.writer
            .into_inner()
            .map_err(|e| format!("Avro flush failed: {}", e))?;
        Ok(())
    }
}

fn build_schema(channel_names: &[String]) -> Result<Schema, String> {
    let mut fields = Vec::with_capacity(channel_names.len() + 1);
    fields.push(r#"{"name":"timestamp_s","type":"double"}"#.to_string());

    for (i, name) in channel_names.iter().enumerate() {
        // Use ch_N as the Avro field name (safe identifier), store display name as alias
        let safe_name = format!("ch_{}", i);
        let escaped = name.replace('\\', "\\\\").replace('"', "\\\"");
        fields.push(format!(
            r#"{{"name":"{}","type":"double","doc":"{}"}}"#,
            safe_name, escaped
        ));
    }

    let fields_str = fields.join(",");
    let schema_json = format!(
        r#"{{"type":"record","name":"DaqSample","namespace":"mprofiler.daq","fields":[{fields_str}]}}"#,
    );

    Schema::parse_str(&schema_json).map_err(|e| {
        error!("DAQ: Failed to parse Avro schema: {}", e);
        format!("Avro schema error: {}", e)
    })
}
