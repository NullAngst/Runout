//! Source audio held in memory, decoding through symphonia, WAV writing.

use crate::{Error, Result};
use std::path::Path;

pub const PEAK_BUCKET: usize = 256;

pub enum Storage {
    I16(Vec<i16>),
    F32(Vec<f32>),
}

/// One decoded source (a side, or one imported file). Never modified.
pub struct SourceAudio {
    pub rate: u32,
    pub channels: usize,
    pub bits: u16,
    pub frames: usize,
    data: Storage,
    /// min/max per PEAK_BUCKET frames, all channels combined.
    pub peaks: Vec<(f32, f32)>,
}

impl SourceAudio {
    pub fn from_interleaved_f32(rate: u32, channels: usize, bits: u16, data: Vec<f32>) -> Self {
        let frames = data.len() / channels.max(1);
        let mut s = SourceAudio { rate, channels, bits, frames, data: Storage::F32(data), peaks: Vec::new() };
        s.build_peaks();
        s
    }

    pub fn from_interleaved_i16(rate: u32, channels: usize, data: Vec<i16>) -> Self {
        let frames = data.len() / channels.max(1);
        let mut s = SourceAudio { rate, channels, bits: 16, frames, data: Storage::I16(data), peaks: Vec::new() };
        s.build_peaks();
        s
    }

    pub fn from_planar(rate: u32, bits: u16, ch: &[Vec<f32>]) -> Self {
        let channels = ch.len();
        let frames = ch.first().map(|c| c.len()).unwrap_or(0);
        let mut data = Vec::with_capacity(frames * channels);
        for i in 0..frames {
            for c in ch {
                data.push(c[i]);
            }
        }
        Self::from_interleaved_f32(rate, channels, bits, data)
    }

    pub fn duration(&self) -> f64 {
        self.frames as f64 / self.rate as f64
    }

    #[inline]
    pub fn sample(&self, frame: usize, ch: usize) -> f32 {
        let i = frame * self.channels + ch;
        match &self.data {
            Storage::I16(v) => v[i] as f32 / 32768.0,
            Storage::F32(v) => v[i],
        }
    }

    /// Read one channel for frames [from, to); zeros outside the source.
    pub fn read_channel(&self, ch: usize, from: i64, to: i64) -> Vec<f32> {
        let len = (to - from).max(0) as usize;
        let mut out = vec![0.0f32; len];
        let lo = from.max(0);
        let hi = to.min(self.frames as i64);
        if hi <= lo {
            return out;
        }
        let ch = ch.min(self.channels - 1);
        let off = (lo - from) as usize;
        let c = self.channels;
        match &self.data {
            Storage::I16(v) => {
                for (j, f) in (lo as usize..hi as usize).enumerate() {
                    out[off + j] = v[f * c + ch] as f32 / 32768.0;
                }
            }
            Storage::F32(v) => {
                for (j, f) in (lo as usize..hi as usize).enumerate() {
                    out[off + j] = v[f * c + ch];
                }
            }
        }
        out
    }

    pub fn read(&self, from: i64, to: i64) -> Vec<Vec<f32>> {
        (0..self.channels).map(|c| self.read_channel(c, from, to)).collect()
    }

    /// Mono mix of a range.
    pub fn read_mono(&self, from: i64, to: i64) -> Vec<f32> {
        let chans = self.read(from, to);
        let n = chans[0].len();
        let k = 1.0 / self.channels as f32;
        (0..n).map(|i| chans.iter().map(|c| c[i]).sum::<f32>() * k).collect()
    }

    fn build_peaks(&mut self) {
        let nb = self.frames.div_ceil(PEAK_BUCKET);
        let mut peaks = Vec::with_capacity(nb);
        for b in 0..nb {
            let f0 = b * PEAK_BUCKET;
            let f1 = ((b + 1) * PEAK_BUCKET).min(self.frames);
            let mut lo = 0.0f32;
            let mut hi = 0.0f32;
            for f in f0..f1 {
                for c in 0..self.channels {
                    let v = self.sample(f, c);
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
            }
            peaks.push((lo, hi));
        }
        self.peaks = peaks;
    }

    pub fn is_i16(&self) -> bool {
        matches!(self.data, Storage::I16(_))
    }

    /// Exact integer samples for a range (for bit-exact pass-through export).
    pub fn read_int_interleaved(&self, from: usize, to: usize, bits: u16) -> Vec<i32> {
        let c = self.channels;
        let scale = (1i64 << (bits - 1)) as f64;
        let mut out = Vec::with_capacity((to - from) * c);
        match &self.data {
            Storage::I16(v) => {
                let shift = bits as i32 - 16;
                for i in from * c..to * c {
                    let s = v[i] as i32;
                    out.push(if shift >= 0 { s << shift } else { s >> (-shift) });
                }
            }
            Storage::F32(v) => {
                for i in from * c..to * c {
                    let s = (v[i] as f64 * scale).round().clamp(-scale, scale - 1.0);
                    out.push(s as i32);
                }
            }
        }
        out
    }
}

/// Decode any supported file (WAV, FLAC, AIFF, OGG, MP3) into memory.
pub fn decode_file(path: &Path) -> Result<SourceAudio> {
    use symphonia::core::codecs::audio::AudioDecoderOptions;
    use symphonia::core::formats::probe::Hint;
    use symphonia::core::formats::{FormatOptions, TrackType};
    use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
    use symphonia::core::meta::MetadataOptions;

    let file = std::fs::File::open(path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
    let mss = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }
    let mut format = symphonia::default::get_probe()
        .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
        .map_err(|e| Error::Decode(format!("{}: {e}", path.display())))?;
    let track = format
        .default_track(TrackType::Audio)
        .ok_or_else(|| Error::Decode(format!("{}: no audio track", path.display())))?;
    let track_id = track.id;
    let params = track
        .codec_params
        .as_ref()
        .and_then(|p| p.audio())
        .cloned()
        .ok_or_else(|| Error::Decode("no audio codec parameters".into()))?;
    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(|e| Error::Decode(format!("{}: {e}", path.display())))?;
    let bits = params.bits_per_sample.unwrap_or(0) as u16;
    let lossy = bits == 0;
    let want_i16 = bits > 0 && bits <= 16;

    let mut rate = params.sample_rate.unwrap_or(0);
    let mut channels = params.channels.as_ref().map(|c| c.count()).unwrap_or(0);
    let mut i16buf: Vec<i16> = Vec::new();
    let mut f32buf: Vec<f32> = Vec::new();
    let mut tmp16: Vec<i16> = Vec::new();
    let mut tmp32: Vec<f32> = Vec::new();
    loop {
        let packet = match format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => break,
            Err(symphonia::core::errors::Error::IoError(_)) => break,
            Err(e) => return Err(Error::Decode(format!("{}: {e}", path.display()))),
        };
        if packet.track_id != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(buf) => {
                if rate == 0 {
                    rate = buf.spec().rate();
                }
                if channels == 0 {
                    channels = buf.spec().channels().count();
                }
                if want_i16 {
                    buf.copy_to_vec_interleaved(&mut tmp16);
                    i16buf.extend_from_slice(&tmp16);
                } else {
                    buf.copy_to_vec_interleaved(&mut tmp32);
                    f32buf.extend_from_slice(&tmp32);
                }
            }
            Err(symphonia::core::errors::Error::DecodeError(_)) => continue,
            Err(symphonia::core::errors::Error::ResetRequired) => {
                decoder.reset();
                continue;
            }
            Err(e) => return Err(Error::Decode(format!("{}: {e}", path.display()))),
        }
    }
    if rate == 0 || channels == 0 {
        return Err(Error::Decode(format!("{}: could not read sample rate or channels", path.display())));
    }
    Ok(if want_i16 {
        SourceAudio::from_interleaved_i16(rate, channels, i16buf)
    } else {
        SourceAudio::from_interleaved_f32(rate, channels, if lossy { 16 } else { bits.max(16) }, f32buf)
    })
}

/// Write planar float audio as a WAV of the given bit depth (16 or 24, or 32 float).
pub fn write_wav(path: &Path, rate: u32, bits: u16, ch: &[Vec<f32>]) -> Result<()> {
    let spec = hound::WavSpec {
        channels: ch.len() as u16,
        sample_rate: rate,
        bits_per_sample: bits,
        sample_format: if bits == 32 { hound::SampleFormat::Float } else { hound::SampleFormat::Int },
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| Error::Io(e.to_string()))?;
    let n = ch.first().map(|c| c.len()).unwrap_or(0);
    let scale = if bits == 32 { 1.0 } else { (1i64 << (bits - 1)) as f64 };
    for i in 0..n {
        for c in ch {
            if bits == 32 {
                w.write_sample(c[i]).map_err(|e| Error::Io(e.to_string()))?;
            } else {
                let s = (c[i] as f64 * scale).round().clamp(-scale, scale - 1.0) as i32;
                w.write_sample(s).map_err(|e| Error::Io(e.to_string()))?;
            }
        }
    }
    w.finalize().map_err(|e| Error::Io(e.to_string()))?;
    Ok(())
}

/// Write a source back out as WAV at its own bit depth, sample for sample.
pub fn write_source_wav(path: &Path, src: &SourceAudio) -> Result<()> {
    let bits = if src.bits <= 16 { 16 } else { 24 };
    let spec = hound::WavSpec {
        channels: src.channels as u16,
        sample_rate: src.rate,
        bits_per_sample: bits,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| Error::Io(e.to_string()))?;
    let step = 1 << 16;
    let mut f = 0;
    while f < src.frames {
        let e = (f + step).min(src.frames);
        for s in src.read_int_interleaved(f, e, bits) {
            w.write_sample(s).map_err(|e| Error::Io(e.to_string()))?;
        }
        f = e;
    }
    w.finalize().map_err(|e| Error::Io(e.to_string()))?;
    Ok(())
}

/// Fix the RIFF and data chunk sizes of a WAV whose writer died before
/// finalizing (a crash mid-capture). Returns true if the header was changed.
pub fn repair_wav_header(path: &Path) -> Result<bool> {
    use std::io::{Read, Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(path).map_err(|e| Error::Io(e.to_string()))?;
    let len = f.metadata().map_err(|e| Error::Io(e.to_string()))?.len();
    let mut head = vec![0u8; 4096.min(len as usize)];
    f.read_exact(&mut head).map_err(|e| Error::Io(e.to_string()))?;
    if head.len() < 44 || &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return Err(Error::Decode("not a WAV file".into()));
    }
    // walk chunks to find "data"
    let mut pos = 12usize;
    let mut block_align = 4u64;
    while pos + 8 <= head.len() {
        let id = &head[pos..pos + 4];
        let size = u32::from_le_bytes([head[pos + 4], head[pos + 5], head[pos + 6], head[pos + 7]]) as usize;
        if id == b"fmt " && pos + 8 + 14 <= head.len() {
            block_align = u16::from_le_bytes([head[pos + 8 + 12], head[pos + 8 + 13]]) as u64;
        }
        if id == b"data" {
            let data_start = pos as u64 + 8;
            let mut data_len = len - data_start;
            data_len -= data_len % block_align.max(1);
            let riff = (data_start + data_len - 8) as u32;
            let old_data = size as u64;
            if old_data == data_len && u32::from_le_bytes([head[4], head[5], head[6], head[7]]) == riff {
                return Ok(false);
            }
            f.seek(SeekFrom::Start(4)).map_err(|e| Error::Io(e.to_string()))?;
            f.write_all(&riff.to_le_bytes()).map_err(|e| Error::Io(e.to_string()))?;
            f.seek(SeekFrom::Start(pos as u64 + 4)).map_err(|e| Error::Io(e.to_string()))?;
            f.write_all(&(data_len as u32).to_le_bytes()).map_err(|e| Error::Io(e.to_string()))?;
            return Ok(true);
        }
        pos += 8 + size + (size & 1);
    }
    Err(Error::Decode("WAV has no data chunk".into()))
}
