//! Streaming recovery with P1/P2 and P6-first timing hypotheses.
//! Uplink AP is not a free-standing checksum: blind addresses remain candidates.
use crate::{
    decode::{decode_frame, util::SpecialDesignator, DecodedUplink, UplinkFormat},
    dsp::{
        detector::{Detection, Detector},
        timing::UplinkTiming,
    },
};
use num_complex::Complex32;
use serde::Serialize;
use std::collections::{HashSet, VecDeque};

#[derive(Debug, Clone, Serialize)]
pub struct Recovered {
    pub detection: Detection,
    pub detection_method: &'static str,
    pub demodulation_offset_samples: isize,
    pub phase_rotation_rad: f32,
    pub decision_confidence: f32,
    pub symbol_coherence: f32,
    pub sync_correlation: f32,
    pub address_validation: &'static str,
    pub validation: &'static str,
    pub message: DecodedUplink,
}
impl Recovered {
    fn score(&self) -> f32 {
        self.decision_confidence + self.symbol_coherence * 0.5 + (-self.sync_correlation) * 0.2
    }
}

pub struct Receiver {
    timing: UplinkTiming,
    known: Option<HashSet<u32>>,
    known_only: bool,
    confidence: f32,
    amplitude: f32,
    p6_first: bool,
    buffer: Vec<Complex32>,
    base: usize,
    processed_until: usize,
    recent: VecDeque<(usize, String)>,
}
impl Receiver {
    pub fn new(
        timing: UplinkTiming,
        known: Option<HashSet<u32>>,
        confidence: f32,
        amplitude: f32,
        p6_first: bool,
    ) -> Self {
        Self {
            timing,
            known,
            known_only: false,
            confidence,
            amplitude,
            p6_first,
            buffer: vec![],
            base: 0,
            processed_until: 0,
            recent: VecDeque::new(),
        }
    }
    /// Explicitly restrict addressed hypotheses to known addresses. All-calls
    /// remain eligible. By default the address list only annotates results.
    pub fn with_known_only(mut self, enabled: bool) -> Self {
        self.known_only = enabled;
        self
    }
    /// Empty final call flushes frames held for long-format lookahead.
    pub fn push(&mut self, chunk: &[Complex32], final_chunk: bool) -> Vec<Recovered> {
        self.buffer.extend_from_slice(chunk);
        let t = self.timing;
        let lookahead = t.min_samples_long() + 8 * t.samples_per_bit;
        let end = self.base + self.buffer.len();
        let safe_end = if final_chunk {
            end
        } else {
            end.saturating_sub(lookahead)
        };
        let mut detections: Vec<_> = Detector::with_timing(3., false, t)
            .detect(&self.buffer, self.base)
            .into_iter()
            .map(|d| (d, "p1_p2"))
            .collect();
        if self.p6_first {
            detections.extend(p6_detections(&self.buffer, self.base, t, self.amplitude));
        }
        let mut recovered = vec![];
        for (d, method) in detections {
            if d.p6_sample < self.processed_until || d.p6_sample >= safe_end {
                continue;
            }
            if let Some(r) = self.recover(d, method) {
                recovered.push(r);
            }
        }
        recovered.sort_by_key(|r| r.detection.p6_sample);
        // Multiple onset hypotheses for one physical P6: prefer waveform quality,
        // rather than emitting arbitrary alternate addresses from timing offsets.
        let mut grouped: Vec<Recovered> = vec![];
        for r in recovered {
            if let Some(last) = grouped.last_mut() {
                if r.detection.p6_sample - last.detection.p6_sample <= 8 * t.samples_per_bit {
                    if r.score() > last.score() {
                        *last = r;
                    }
                    continue;
                }
            }
            grouped.push(r);
        }
        let tolerance = (t.sample_rate_hz as usize) / 10_000; // 100us, same target/raw only.
        grouped.retain(|r| {
            let time = r.detection.p6_sample;
            while self
                .recent
                .front()
                .is_some_and(|(s, _)| time.saturating_sub(*s) > tolerance)
            {
                self.recent.pop_front();
            }
            if self
                .recent
                .iter()
                .any(|(s, raw)| s.abs_diff(time) <= tolerance && raw == &r.message.raw)
            {
                false
            } else {
                self.recent.push_back((time, r.message.raw.clone()));
                true
            }
        });
        self.processed_until = safe_end;
        if !final_chunk {
            let keep = 2 * lookahead;
            if self.buffer.len() > keep {
                let drain = self.buffer.len() - keep;
                self.buffer.drain(..drain);
                self.base += drain;
            }
        }
        grouped
    }

    fn recover(&self, nominal: Detection, method: &'static str) -> Option<Recovered> {
        let t = self.timing;
        let spb = t.samples_per_bit;
        let local = nominal.p6_sample - self.base;
        let mut best: Option<Recovered> = None;
        let mut offsets = vec![0isize];
        let extent = if method == "p6" { 4 * spb } else { 2 * spb };
        for n in 1..=extent {
            offsets.extend([-(n as isize), n as isize]);
        }
        for offset in offsets {
            let Some(p6) = local.checked_add_signed(offset) else {
                continue;
            };
            let data = p6 + t.p6_data_offset_samples;
            if p6 < 3 * spb || p6 + 5 * spb > self.buffer.len() {
                continue;
            }
            // A sync-like phase change at P1/P2 or inside data is not a P6
            // onset. Require a quiet preceding gap and sustained pre-sync
            // energy. This is especially important without an address list.
            let mean_mag = |a: usize, b: usize| {
                self.buffer[a..b].iter().map(|s| s.norm()).sum::<f32>() / (b - a) as f32
            };
            let leading = mean_mag(p6 + spb, p6 + 4 * spb);
            let late_leading = mean_mag(p6 + 3 * spb, p6 + 4 * spb);
            let gap = mean_mag(p6 - 3 * spb, p6 - spb);
            if leading < nominal.noise_mag * 2.0
                || late_leading < leading * 0.5
                || gap > leading * 0.75
            {
                continue;
            }
            for bits in [56usize, 112] {
                if data + bits * spb > self.buffer.len() {
                    continue;
                }
                // A coherent noise/DC tail must not turn a short frame into
                // a spurious 112-bit UF24 hypothesis.
                if bits == 112 && mean_mag(data + 56 * spb, data + 112 * spb) < leading * 0.4 {
                    continue;
                }
                let mut c = Vec::with_capacity(bits);
                let mut coherence = 0.0;
                for bit in 0..bits {
                    let start = data + bit * spb;
                    let correlation = (0..spb)
                        .map(|n| self.buffer[start + n] * self.buffer[start + n - spb].conj())
                        .sum::<Complex32>();
                    let pre = (0..spb)
                        .map(|n| self.buffer[start + n - spb].norm_sqr())
                        .sum::<f32>();
                    let post = (0..spb)
                        .map(|n| self.buffer[start + n].norm_sqr())
                        .sum::<f32>();
                    coherence += correlation.norm() / (pre * post).sqrt().max(1e-12);
                    c.push(correlation);
                }
                let doubled = c.iter().map(|z| z * z).sum::<Complex32>();
                let phase = 0.5 * doubled.arg();
                for rotation in [0., phase] {
                    let mut rot = Complex32::from_polar(1., -rotation);
                    let sync = p6 + t.p6_sync_samples;
                    let mut sync_corr = Complex32::default();
                    let mut pre = 0.;
                    let mut post = 0.;
                    for n in 0..spb {
                        let a = self.buffer[sync + n];
                        let b = self.buffer[sync + n - spb];
                        sync_corr += a * b.conj();
                        pre += b.norm_sqr();
                        post += a.norm_sqr();
                    }
                    let mut sync_real = (sync_corr * rot).re / (pre * post).sqrt().max(1e-12);
                    if !sync_real.is_finite() {
                        continue;
                    }
                    // Resolve the squared-correlation π ambiguity using P6 sync,
                    // never by guessing an arbitrary target address.
                    let inverted = sync_real > 0.;
                    if inverted {
                        rot = -rot;
                        sync_real = -sync_real;
                    }
                    if sync_real > -0.15 {
                        continue;
                    }
                    let confidence = c
                        .iter()
                        .map(|z| (z * rot).re.abs() / z.norm().max(1e-12))
                        .sum::<f32>()
                        / bits as f32;
                    if !confidence.is_finite()
                        || !coherence.is_finite()
                        || confidence < self.confidence
                    {
                        continue;
                    }
                    let mut frame = vec![0u8; bits / 8];
                    for (n, z) in c.iter().enumerate() {
                        if (z * rot).re < 0. {
                            frame[n / 8] |= 1 << (7 - n % 8);
                        }
                    }
                    // These hypotheses were already rejected downstream;
                    // avoid CRC/Deku/hex allocations for unsupported UFs or
                    // impossible format/length combinations.
                    if !matches!(
                        (frame[0] >> 3, bits),
                        (0 | 4 | 5 | 11, 56) | (16 | 20 | 21 | 24..=31, 112)
                    ) {
                        continue;
                    }
                    let Ok(message) = decode_frame(&frame) else {
                        continue;
                    };
                    if !plausible(&message.payload) {
                        continue;
                    }
                    let Some(address) = message.icao24() else {
                        continue;
                    };
                    if address == 0 {
                        continue;
                    }
                    let all_call = matches!(message.payload, UplinkFormat::Uf11(_));
                    if all_call && address != 0xffffff {
                        continue;
                    }
                    let known_match = self.known.as_ref().is_some_and(|k| k.contains(&address));
                    if !all_call && self.known_only && !known_match {
                        continue;
                    }
                    let mut detection = nominal.clone();
                    detection.p6_sample = self.base + p6;
                    detection.num_bits = bits;
                    let recovered = Recovered {
                        detection,
                        detection_method: method,
                        demodulation_offset_samples: offset,
                        phase_rotation_rad: rotation
                            + if inverted { std::f32::consts::PI } else { 0. },
                        decision_confidence: confidence,
                        symbol_coherence: coherence / bits as f32,
                        sync_correlation: sync_real,
                        address_validation: if all_call {
                            "all_call_address"
                        } else if known_match {
                            "known_address_match"
                        } else {
                            "unverified"
                        },
                        validation: if all_call {
                            "crc_valid_all_call"
                        } else {
                            "candidate"
                        },
                        message,
                    };
                    if best.as_ref().is_none_or(|b| recovered.score() > b.score()) {
                        best = Some(recovered);
                    }
                }
            }
        }
        best
    }
}

fn sd_valid(sd: &SpecialDesignator) -> bool {
    match sd {
        SpecialDesignator::Di0(s) => s.reserved_21_27 == 0 && s.reserved_29_32 == 0,
        SpecialDesignator::Di1(_) => true,
        SpecialDesignator::Di2(s) => s.reserved_17_20 == 0 && s.reserved_29_32 == 0,
        SpecialDesignator::Di3(s) => s.reserved_29_32 == 0,
        SpecialDesignator::Di7(s) => !s.reserved_25 && !s.reserved_27,
        SpecialDesignator::Reserved { .. } => false,
    }
}
fn pc_valid(pc: u8) -> bool {
    !matches!(pc, 2 | 3 | 7)
}
fn plausible(payload: &UplinkFormat) -> bool {
    match payload {
        UplinkFormat::Uf0(s) => s.spare0 == 0 && s.spare1 == 0 && s.spare2 == 0,
        UplinkFormat::Uf4(s) => pc_valid(s.pc) && sd_valid(&s.sd),
        UplinkFormat::Uf5(s) => pc_valid(s.pc) && sd_valid(&s.sd),
        UplinkFormat::Uf11(s) => s.spare == 0 && s.cl <= 4 && !(s.cl == 1 && s.ic == 0),
        UplinkFormat::Uf16(s) => s.spare0 == 0 && s.spare1 == 0 && s.spare2 == 0,
        UplinkFormat::Uf20(s) => pc_valid(s.pc) && sd_valid(&s.sd),
        UplinkFormat::Uf21(s) => pc_valid(s.pc) && sd_valid(&s.sd),
        UplinkFormat::Uf24(_) => true,
        UplinkFormat::Unknown { .. } => false,
    }
}

fn p6_detections(
    iq: &[Complex32],
    base: usize,
    t: UplinkTiming,
    floor: f32,
) -> Vec<(Detection, &'static str)> {
    let spb = t.samples_per_bit;
    let width = 2 * spb;
    let short_extent = t.p6_data_offset_samples + 56 * spb;
    if iq.len() <= short_extent + width + 1 {
        return vec![];
    }
    let mut sums = Vec::with_capacity(iq.len() + 1);
    sums.push(0f64);
    for s in iq {
        sums.push(sums.last().unwrap() + s.norm() as f64);
    }
    let avg = |center: usize, size: usize| {
        let start = center.saturating_sub(size / 2);
        let end = (start + size).min(iq.len());
        ((sums[end] - sums[start]) / (end - start) as f64) as f32
    };
    let stride = (iq.len() / 2048).max(1);
    let mut noise: Vec<_> = (width..iq.len() - width)
        .step_by(stride)
        .map(|i| avg(i, width))
        .collect();
    let midpoint = noise.len() / 2;
    let median = *noise.select_nth_unstable_by(midpoint, f32::total_cmp).1;
    let threshold = floor.max(median * 3.);
    let mut out = vec![];
    let mut previous = avg(width, width);
    let mut last_edge = 0;
    for i in width + 1..=iq.len() - short_extent {
        let current = avg(i, width);
        if current > threshold
            && previous <= threshold
            && i > 10 * spb
            && i - last_edge > 3 * spb
            && avg(i + 30 * spb, 45 * spb) > threshold
        {
            last_edge = i;
            for anchor in [i, i + t.p6_offset_samples] {
                out.push((
                    Detection {
                        p1_sample: (base + anchor).saturating_sub(t.p6_offset_samples),
                        p6_sample: base + anchor,
                        num_bits: 56,
                        signal_power: current * current,
                        noise_mag: median,
                        threshold_mag: threshold,
                    },
                    "p6",
                ));
            }
        }
        previous = current;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Independent polynomial long division for parity, not the receiver helper.
    fn frame(prefix: &[u8], addr: u32) -> Vec<u8> {
        let mut crc = 0u32;
        for n in 0..prefix.len() * 8 + 24 {
            let bit = if n < prefix.len() * 8 {
                (prefix[n / 8] >> (7 - n % 8)) & 1
            } else {
                0
            };
            crc = (crc << 1) | bit as u32;
            if crc & 0x1000000 != 0 {
                crc ^= 0x1fff409;
            }
        }
        let mut product = 0u64;
        for i in 0..24 {
            if addr & (1 << i) != 0 {
                product ^= 0x1fff409u64 << i;
            }
        }
        let parity = crc ^ ((product >> 24) & 0xffffff) as u32;
        let mut v = prefix.to_vec();
        v.extend_from_slice(&parity.to_be_bytes()[1..]);
        v
    }
    fn waveform(frame: &[u8], timing: UplinkTiming) -> Vec<Complex32> {
        let p1 = 200;
        let p6 = p1 + timing.p6_offset_samples;
        let start = p6 + timing.p6_data_offset_samples;
        let mut iq = vec![
            Complex32::new(0.0001, 0.0002);
            start + frame.len() * 8 * timing.samples_per_bit + 1000
        ];
        iq[p1..p1 + timing.p1_samples].fill(Complex32::new(0.1, 0.));
        iq[p1 + timing.p2_offset_samples..p1 + timing.p2_offset_samples + timing.p2_samples]
            .fill(Complex32::new(0.1, 0.));
        let mut sign = 1.;
        for (i, sample) in iq.iter_mut().enumerate().take(start).skip(p6) {
            if i == p6 + timing.p6_sync_samples {
                sign = -sign;
            }
            *sample = Complex32::new(0.1 * sign, 0.);
        }
        for n in 0..frame.len() * 8 {
            if frame[n / 8] & (1 << (7 - n % 8)) != 0 {
                sign = -sign;
            }
            for j in 0..timing.samples_per_bit {
                iq[start + n * timing.samples_per_bit + j] = Complex32::new(0.1 * sign, 0.);
            }
        }
        iq
    }
    #[test]
    fn native_recovery_without_whitelist_and_across_boundaries() {
        for rate in [8_000_000, 12_000_000, 16_000_000, 20_000_000] {
            let t = UplinkTiming::from_sample_rate(rate).unwrap();
            let f = frame(&[0x20, 0xa9, 0x30, 0x40], 0x39bda4);
            let iq = waveform(&f, t);
            let mut receiver = Receiver::new(t, None, 0.9, 0.025, true);
            let mut found = vec![];
            for chunk in iq.chunks(113) {
                found.extend(receiver.push(chunk, false));
            }
            found.extend(receiver.push(&[], true));
            assert!(
                found
                    .iter()
                    .any(|r| r.message.raw == crate::bits::bytes_to_hex(&f)),
                "rate {rate}: {found:?}"
            );
            assert_eq!(
                found
                    .iter()
                    .filter(|r| r.message.raw == crate::bits::bytes_to_hex(&f))
                    .count(),
                1
            );
            assert!(found.iter().all(|r| r.validation == "candidate"));
        }
    }
    #[test]
    fn uf0_known_annotation_and_bad_spares() {
        let t = UplinkTiming::default();
        let f = frame(&[0, 0, 0, 0], 0x46c942);
        let iq = waveform(&f, t);
        let mut known = HashSet::new();
        known.insert(0x46c942);
        let mut r = Receiver::new(t, Some(known), 0.9, 0.025, true);
        let found = r.push(&iq, true);
        assert!(found
            .iter()
            .any(|r| r.message.raw == crate::bits::bytes_to_hex(&f)
                && r.address_validation == "known_address_match"));
        let bad = decode_frame(&frame(
            &[
                0x85, 0x13, 0xdb, 0xb8, 0x38, 0x1b, 0xed, 0xbe, 0xc2, 0xb3, 0xff,
            ],
            0x46c942,
        ))
        .unwrap();
        assert!(!plausible(&bad.payload));
    }
    #[test]
    fn unknown_addresses_are_retained_unless_known_only() {
        let t = UplinkTiming::default();
        let f = frame(&[0, 0, 0, 0], 0x46c942);
        let iq = waveform(&f, t);
        let list = Some(HashSet::from([0x39bda4]));
        let mut blind = Receiver::new(t, None, 0.9, 0.025, true);
        let baseline = blind.push(&iq, true);
        let mut annotated = Receiver::new(t, list.clone(), 0.9, 0.025, true);
        let events = annotated.push(&iq, true);
        assert_eq!(format!("{events:?}"), format!("{baseline:?}"));
        assert!(events
            .iter()
            .any(|r| r.message.raw == crate::bits::bytes_to_hex(&f)
                && r.address_validation == "unverified"
                && r.validation == "candidate"));
        let mut filtered = Receiver::new(t, list, 0.9, 0.025, true).with_known_only(true);
        assert!(filtered.push(&iq, true).is_empty());
        let mut matching = Receiver::new(t, Some(HashSet::from([0x46c942])), 0.9, 0.025, true)
            .with_known_only(true);
        assert!(matching
            .push(&iq, true)
            .iter()
            .any(|r| r.message.raw == crate::bits::bytes_to_hex(&f)
                && r.address_validation == "known_address_match"
                && r.validation == "candidate"));
    }

    #[test]
    fn long_frame_retains_full_payload_and_ap() {
        let t = UplinkTiming::from_sample_rate(16_000_000).unwrap();
        let f = frame(&[0xa0, 0xa9, 0x30, 0x40, 0x7f, 1, 2, 3, 4, 5, 6], 0x39bda4);
        let mut r = Receiver::new(t, None, 0.9, 0.025, true);
        let events = r.push(&waveform(&f, t), true);
        assert!(events
            .iter()
            .any(|r| r.message.raw == crate::bits::bytes_to_hex(&f)
                && r.detection.num_bits == 112
                && r.address_validation == "unverified"));
    }

    #[test]
    fn all_call_fixed_address_and_p6_only_eof() {
        let t = UplinkTiming::from_sample_rate(16_000_000).unwrap();
        let f = frame(&[0x58, 0x98, 0, 0], 0xffffff);
        let mut iq = waveform(&f, t);
        iq[200..200 + t.p1_samples].fill(Complex32::new(0.0001, 0.0002));
        iq[200 + t.p2_offset_samples..200 + t.p2_offset_samples + t.p2_samples]
            .fill(Complex32::new(0.0001, 0.0002));
        iq.truncate(
            200 + t.p6_offset_samples + t.p6_data_offset_samples + 56 * t.samples_per_bit + 4,
        );
        let mut padded = vec![Complex32::new(0.0001, 0.0002); 1000];
        padded.extend(iq);
        for (n, s) in padded.iter_mut().enumerate() {
            *s *= Complex32::from_polar(1., 0.11 * n as f32);
        }
        let mut r = Receiver::new(t, Some(HashSet::from([0x39bda4])), 0.9, 0.025, true)
            .with_known_only(true);
        let events = r.push(&padded, true);
        assert!(events
            .iter()
            .any(|r| r.message.raw == crate::bits::bytes_to_hex(&f)
                && r.validation == "crc_valid_all_call"
                && r.detection_method == "p6"));
    }

    #[test]
    fn noise_and_empty_data_do_not_decode() {
        let mut r = Receiver::new(UplinkTiming::default(), None, 0.9, 0.025, true);
        assert!(r
            .push(&vec![Complex32::new(0.001, 0.); 10000], true)
            .is_empty());
        assert!(r.push(&[], true).is_empty());
    }
}
