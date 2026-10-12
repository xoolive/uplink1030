//! Host-clock estimates, not hardware-synchronized RF timestamps.
//! Keep input-chunk anchors until decoder/resampler lookahead has been emitted.
use std::{
    collections::VecDeque,
    time::{SystemTime, UNIX_EPOCH},
};

pub fn system_now() -> f64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(t) => t.as_secs_f64(),
        Err(e) => -e.duration().as_secs_f64(),
    }
}

struct Arrival {
    start: usize,
    kept_end: usize,
    read_end: usize,
    time: f64,
}

pub struct ArrivalClock {
    input_rate: u32,
    retain_samples: usize,
    chunks: VecDeque<Arrival>,
}

impl ArrivalClock {
    pub fn new(input_rate: u32, retain_samples: usize) -> Self {
        assert!(input_rate > 0 && retain_samples > 0);
        Self {
            input_rate,
            retain_samples,
            chunks: VecDeque::new(),
        }
    }

    /// Capture delivery time before DSP. `read_len` includes samples discarded
    /// by --seconds: arrival belongs to the end of the complete delivered chunk.
    pub fn record(&mut self, start: usize, read_len: usize, kept_len: usize, time: f64) {
        assert!(kept_len <= read_len);
        if kept_len == 0 {
            return;
        }
        self.chunks.push_back(Arrival {
            start,
            kept_end: start + kept_len,
            read_end: start + read_len,
            time,
        });
    }

    pub fn estimate(&self, decoder_sample: usize, decoder_rate: u32) -> Option<f64> {
        // Compare rational input coordinates exactly, including fractional
        // centers after rate conversion and chunk-boundary samples.
        let position = decoder_sample as u128 * self.input_rate as u128;
        let scale = decoder_rate as u128;
        self.chunks
            .iter()
            .rev()
            .find(|c| c.start as u128 * scale <= position && position < c.kept_end as u128 * scale)
            .map(|c| {
                let remaining = c.read_end as u128 * scale - position;
                c.time - remaining as f64 / (decoder_rate as f64 * self.input_rate as f64)
            })
    }

    /// Prune only AFTER processing this chunk's events; some events belong to
    /// the preceding chunk and are emitted late due to frame/FIR lookahead.
    pub fn prune(&mut self, processed_input: usize) {
        let cutoff = processed_input.saturating_sub(self.retain_samples);
        while self.chunks.front().is_some_and(|c| c.kept_end <= cutoff) {
            self.chunks.pop_front();
        }
    }
}

pub struct EventContext<'a> {
    pub source: &'a str,
    pub input_rate: u32,
    pub decode_rate: u32,
    pub start: Option<f64>,
    pub basis: &'static str,
    pub arrivals: Option<ArrivalClock>,
}

impl EventContext<'_> {
    pub fn reception_time(&self, decoder_sample: usize) -> Option<f64> {
        if let Some(clock) = &self.arrivals {
            clock.estimate(decoder_sample, self.decode_rate)
        } else {
            self.start
                .map(|start| start + decoder_sample as f64 / self.decode_rate as f64)
        }
    }
    pub fn reception_basis(&self) -> &'static str {
        if self.arrivals.is_some() {
            "host_chunk_arrival_approximate"
        } else {
            self.basis
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn arrival_corrects_remaining_samples_not_elapsed_position() {
        let mut c = ArrivalClock::new(1000, 100);
        c.record(0, 1000, 1000, 100.);
        assert!((c.estimate(200, 1000).unwrap() - 99.2).abs() < 1e-10);
        assert!((c.estimate(800, 1000).unwrap() - 99.8).abs() < 1e-10);
        assert!(c.estimate(1000, 1000).is_none());
    }
    #[test]
    fn delayed_frames_use_original_chunk_and_history_is_bounded() {
        let mut c = ArrivalClock::new(1000, 100);
        c.record(0, 1000, 1000, 100.);
        c.prune(1000);
        c.record(1000, 1000, 1000, 110.);
        assert!((c.estimate(950, 1000).unwrap() - 99.95).abs() < 1e-10);
        assert!((c.estimate(1050, 1000).unwrap() - 109.05).abs() < 1e-10);
        c.prune(2000);
        assert_eq!(c.chunks.len(), 1);
        for i in 2000..10000 {
            c.record(i, 1, 1, 110.);
            c.prune(i + 1);
        }
        assert_eq!(c.chunks.len(), 100);
    }
    #[test]
    fn fractional_resampling_truncation_and_empty_eof() {
        let mut c = ArrivalClock::new(10_000_000, 1000);
        c.record(100, 200, 100, 100.);
        // Decoder index 81 at 8M denotes input coordinate 101.25 at 10M.
        assert!((c.estimate(81, 8_000_000).unwrap() - (100. - 198.75 / 10_000_000.)).abs() < 1e-12);
        c.record(200, 0, 0, 200.);
        assert!((c.estimate(81, 8_000_000).unwrap() - (100. - 198.75 / 10_000_000.)).abs() < 1e-12);
        assert!(c.estimate(160, 8_000_000).is_none());
    }
    #[test]
    fn file_time_is_recording_time_not_replay_wall_clock() {
        let mut ctx = EventContext {
            source: "file",
            input_rate: 16_000_000,
            decode_rate: 8_000_000,
            start: Some(100.),
            basis: "sample_clock_filename_utc",
            arrivals: None,
        };
        assert_eq!(ctx.reception_time(8_000_000), Some(101.));
        assert_eq!(ctx.reception_basis(), "sample_clock_filename_utc");
        ctx.start = None;
        ctx.basis = "relative_samples_only";
        assert!(ctx.reception_time(0).is_none());
    }
}
