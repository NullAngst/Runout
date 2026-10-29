//! Source audio held in memory (or mapped from disk for big WAVs), decoding
//! through symphonia, WAV and RF64 writing.
//!
//! Plain WAV tops out at 4 GB, and a 3-hour reel at 96 kHz / 24-bit stereo is
//! about 6.2 GB. Captures are written by `WavWriter`, which reserves a JUNK
//! chunk up front and turns it into the RF64 ds64 chunk on close when the file
//! has passed 4 GB. symphonia does not read RF64, so RF64 files, and any PCM
//! WAV over 1 GiB, are read here and memory-mapped rather than copied into
//! memory: the OS pages them in as they are played.

use crate::{Error, Result};
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;
use std::sync::Arc;

pub const PEAK_BUCKET: usize = 256;

/// WAVs this big (or any RF64) are mapped instead of decoded into memory.
const MAP_FROM_BYTES: u64 = 1 << 30;

/// Integer PCM straight out of a mapped WAV or RF64 file.
pub struct MappedPcm {
    map: Arc<memmap2::Mmap>,
    off: usize,
    /// Bytes per sample: 2, 3 or 4.
    width: usize,
}

impl MappedPcm {
    #[inline]
    fn int(&self, i: usize) -> i32 {
        let p = self.off + i * self.width;
        let b = &self.map[p..p + self.width];
        match self.width {
            2 => i16::from_le_bytes([b[0], b[1]]) as i32,
            3 => (((b[2] as i32) << 24) | ((b[1] as i32) << 16) | ((b[0] as i32) << 8)) >> 8,
            _ => i32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        }
    }

    #[inline]
    fn f(&self, i: usize) -> f32 {
        match self.width {
            2 => self.int(i) as f32 / 32768.0,
            3 => self.int(i) as f32 / 8388608.0,
            _ => (self.int(i) as f64 / 2147483648.0) as f32,
        }
    }
}

pub enum Storage {
    I16(Vec<i16>),
    F32(Vec<f32>),
    Mapped(MappedPcm),
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
            Storage::Mapped(m) => m.f(i),
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
            Storage::Mapped(m) => {
                for (j, f) in (lo as usize..hi as usize).enumerate() {
                    out[off + j] = m.f(f * c + ch);
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
            Storage::Mapped(m) => {
                let shift = bits as i32 - (m.width as i32 * 8);
                for i in from * c..to * c {
                    let s = m.int(i);
                    out.push(if shift >= 0 { s << shift } else { s >> (-shift) });
                }
            }
        }
        out
    }

    /// Whether the samples are held as 16-bit integers (or mapped from a
    /// 16-bit file): what a bit-exact passthrough needs to know.
    pub fn int_width(&self) -> Option<usize> {
        match &self.data {
            Storage::I16(_) => Some(2),
            Storage::F32(_) => None,
            Storage::Mapped(m) => Some(m.width),
        }
    }
}

/// What a WAV or RF64 header says.
#[derive(Clone, Debug)]
pub struct WavInfo {
    pub rf64: bool,
    pub format: u16,
    pub channels: usize,
    pub rate: u32,
    pub bits: u16,
    pub data_off: u64,
    pub data_len: u64,
}

/// Read the header of a WAV or RF64 file. None for anything else.
pub fn wav_info(path: &Path) -> Option<WavInfo> {
    use std::io::Read;
    let mut f = std::fs::File::open(path).ok()?;
    let flen = f.metadata().ok()?.len();
    let mut head = vec![0u8; 8192.min(flen as usize)];
    f.read_exact(&mut head).ok()?;
    if head.len() < 12 || &head[8..12] != b"WAVE" {
        return None;
    }
    let rf64 = match &head[0..4] {
        b"RIFF" => false,
        b"RF64" => true,
        _ => return None,
    };
    let u16le = |p: usize| u16::from_le_bytes([head[p], head[p + 1]]);
    let u32le = |p: usize| u32::from_le_bytes([head[p], head[p + 1], head[p + 2], head[p + 3]]);
    let u64le = |p: usize| u64::from_le_bytes(head[p..p + 8].try_into().unwrap());
    let mut pos = 12usize;
    let mut ds64_data: Option<u64> = None;
    let (mut format, mut channels, mut rate, mut bits) = (0u16, 0usize, 0u32, 0u16);
    while pos + 8 <= head.len() {
        let id = &head[pos..pos + 4];
        let size = u32le(pos + 4) as usize;
        if id == b"ds64" && pos + 8 + 24 <= head.len() {
            ds64_data = Some(u64le(pos + 8 + 8));
        }
        if id == b"fmt " && pos + 8 + 16 <= head.len() {
            format = u16le(pos + 8);
            channels = u16le(pos + 10) as usize;
            rate = u32le(pos + 12);
            bits = u16le(pos + 22);
            if format == 0xFFFE && pos + 8 + 26 <= head.len() {
                format = u16le(pos + 8 + 24);
            }
        }
        if id == b"data" {
            let off = (pos + 8) as u64;
            let len = if rf64 || size == u32::MAX as usize { ds64_data.unwrap_or(flen - off) } else { size as u64 };
            return Some(WavInfo { rf64, format, channels, rate, bits, data_off: off, data_len: len.min(flen - off) });
        }
        pos += 8 + size + (size & 1);
    }
    None
}

/// Map a PCM WAV or RF64 file (16, 24 or 32-bit integer).
fn open_mapped(path: &Path, w: &WavInfo) -> Result<SourceAudio> {
    let f = std::fs::File::open(path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
    // SAFETY: the file is only read; a capture Runout writes is closed before
    // it is opened here, and sources are never modified.
    let map = unsafe { memmap2::Mmap::map(&f) }.map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
    let width = (w.bits as usize).div_ceil(8);
    if w.format != 1 || !(2..=4).contains(&width) || w.channels == 0 {
        return Err(Error::Decode(format!("{}: only integer PCM is read from RF64", path.display())));
    }
    let frames = (w.data_len as usize) / (width * w.channels);
    let mut s = SourceAudio {
        rate: w.rate,
        channels: w.channels,
        bits: (width * 8) as u16,
        frames,
        data: Storage::Mapped(MappedPcm { map: Arc::new(map), off: w.data_off as usize, width }),
        peaks: Vec::new(),
    };
    s.build_peaks();
    Ok(s)
}

/// Decode any supported file (WAV, RF64, FLAC, AIFF, OGG, MP3). RF64 and big
/// PCM WAVs are mapped; everything else is decoded into memory.
pub fn decode_file(path: &Path) -> Result<SourceAudio> {
    if let Some(w) = wav_info(path) {
        if w.format == 1 && (w.rf64 || w.data_len >= MAP_FROM_BYTES) {
            return open_mapped(path, &w);
        }
    }
    decode_symphonia(path)
}

fn decode_symphonia(path: &Path) -> Result<SourceAudio> {
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

/// Write a source back out as WAV (RF64 past 4 GB) at its own bit depth,
/// sample for sample.
pub fn write_source_wav(path: &Path, src: &SourceAudio) -> Result<()> {
    let bits = if src.bits <= 16 { 16 } else { 24 };
    let mut w = WavWriter::create(path, src.rate, src.channels as u16, bits)?;
    let step = 1 << 16;
    let mut f = 0;
    while f < src.frames {
        let e = (f + step).min(src.frames);
        w.write(&src.read_int_interleaved(f, e, bits))?;
        f = e;
    }
    w.finish()?;
    Ok(())
}

/// Streaming PCM WAV writer that becomes RF64 when it passes 4 GB.
///
/// The header reserves a 28-byte JUNK chunk where the ds64 chunk goes. While
/// the file is under 4 GB it is a plain WAV (`flush_header` keeps its sizes
/// current, so a crash loses seconds, not a side); on `finish`, if it is
/// bigger, RIFF becomes RF64, JUNK becomes ds64 with the 64-bit sizes, and the
/// 32-bit size fields read 0xFFFFFFFF as the RF64 spec says.
pub struct WavWriter {
    f: std::io::BufWriter<std::fs::File>,
    channels: u16,
    width: usize,
    data_bytes: u64,
}

const JUNK_AT: u64 = 12;
const FMT_AT: u64 = JUNK_AT + 8 + 28;
const DATA_HDR_AT: u64 = FMT_AT + 8 + 16;
const DATA_AT: u64 = DATA_HDR_AT + 8;

impl WavWriter {
    /// `bits` 16, 24 or 32 (integer).
    pub fn create(path: &Path, rate: u32, channels: u16, bits: u16) -> Result<Self> {
        let file = std::fs::File::create(path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
        let mut f = std::io::BufWriter::with_capacity(1 << 20, file);
        let width = (bits as usize).div_ceil(8);
        let mut h = Vec::with_capacity(DATA_AT as usize);
        h.extend_from_slice(b"RIFF");
        h.extend_from_slice(&0u32.to_le_bytes());
        h.extend_from_slice(b"WAVE");
        h.extend_from_slice(b"JUNK");
        h.extend_from_slice(&28u32.to_le_bytes());
        h.extend_from_slice(&[0u8; 28]);
        h.extend_from_slice(b"fmt ");
        h.extend_from_slice(&16u32.to_le_bytes());
        h.extend_from_slice(&1u16.to_le_bytes());
        h.extend_from_slice(&channels.to_le_bytes());
        h.extend_from_slice(&rate.to_le_bytes());
        h.extend_from_slice(&(rate * channels as u32 * width as u32).to_le_bytes());
        h.extend_from_slice(&(channels * width as u16).to_le_bytes());
        h.extend_from_slice(&((width * 8) as u16).to_le_bytes());
        h.extend_from_slice(b"data");
        h.extend_from_slice(&0u32.to_le_bytes());
        f.write_all(&h).map_err(|e| Error::Io(e.to_string()))?;
        let mut w = WavWriter { f, channels, width, data_bytes: 0 };
        w.flush_header()?;
        Ok(w)
    }

    /// Interleaved samples at the writer's bit depth.
    pub fn write(&mut self, samples: &[i32]) -> Result<()> {
        let mut buf = Vec::with_capacity(samples.len() * self.width);
        for s in samples {
            let b = s.to_le_bytes();
            buf.extend_from_slice(&b[..self.width]);
        }
        self.f.write_all(&buf).map_err(|e| Error::Io(e.to_string()))?;
        self.data_bytes += buf.len() as u64;
        Ok(())
    }

    pub fn frames(&self) -> u64 {
        self.data_bytes / (self.width as u64 * self.channels.max(1) as u64)
    }

    /// Append `frames` of digital silence without writing them out (the file
    /// gets a sparse hole). For tests of very long captures.
    pub fn write_silence(&mut self, frames: u64) -> Result<()> {
        let io = |e: std::io::Error| Error::Io(e.to_string());
        self.f.flush().map_err(io)?;
        let bytes = frames * self.width as u64 * self.channels as u64;
        let file = self.f.get_mut();
        let end = DATA_AT + self.data_bytes + bytes;
        file.set_len(end).map_err(io)?;
        file.seek(SeekFrom::Start(end)).map_err(io)?;
        self.data_bytes += bytes;
        Ok(())
    }

    /// Write the data so far and fix the size fields (clamped under 4 GB).
    pub fn flush_header(&mut self) -> Result<()> {
        let io = |e: std::io::Error| Error::Io(e.to_string());
        self.f.flush().map_err(io)?;
        let riff = (DATA_AT - 8 + self.data_bytes).min(u32::MAX as u64) as u32;
        let data = self.data_bytes.min(u32::MAX as u64 - DATA_AT) as u32;
        let file = self.f.get_mut();
        file.seek(SeekFrom::Start(4)).map_err(io)?;
        file.write_all(&riff.to_le_bytes()).map_err(io)?;
        file.seek(SeekFrom::Start(DATA_HDR_AT + 4)).map_err(io)?;
        file.write_all(&data.to_le_bytes()).map_err(io)?;
        file.seek(SeekFrom::End(0)).map_err(io)?;
        Ok(())
    }

    /// Close the file, upgrading it to RF64 if it passed 4 GB. Returns frames.
    pub fn finish(mut self) -> Result<u64> {
        let io = |e: std::io::Error| Error::Io(e.to_string());
        if self.data_bytes % 2 == 1 {
            self.f.write_all(&[0]).map_err(io)?;
        }
        self.flush_header()?;
        let frames = self.frames();
        if DATA_AT - 8 + self.data_bytes > u32::MAX as u64 {
            let file = self.f.get_mut();
            upgrade_rf64(file, self.data_bytes, frames).map_err(io)?;
        }
        self.f.flush().map_err(io)?;
        self.f.get_ref().sync_all().map_err(io)?;
        Ok(frames)
    }
}

/// Turn a WAV with the reserved JUNK chunk into RF64.
fn upgrade_rf64(file: &mut std::fs::File, data_bytes: u64, frames: u64) -> std::io::Result<()> {
    file.seek(SeekFrom::Start(0))?;
    file.write_all(b"RF64")?;
    file.write_all(&u32::MAX.to_le_bytes())?;
    file.seek(SeekFrom::Start(JUNK_AT))?;
    file.write_all(b"ds64")?;
    file.write_all(&28u32.to_le_bytes())?;
    file.write_all(&(DATA_AT - 8 + data_bytes + (data_bytes & 1)).to_le_bytes())?;
    file.write_all(&data_bytes.to_le_bytes())?;
    file.write_all(&frames.to_le_bytes())?;
    file.write_all(&0u32.to_le_bytes())?;
    file.seek(SeekFrom::Start(DATA_HDR_AT + 4))?;
    file.write_all(&u32::MAX.to_le_bytes())?;
    file.seek(SeekFrom::End(0))?;
    Ok(())
}

/// Samples of a 24-bit stream (integers at 24 bits) with any of the low 8
/// bits set. Zero for a bit-perfect capture of a 16-bit source.
pub fn low_bits_nonzero(samples: &[i32]) -> usize {
    samples.iter().filter(|v| **v & 0xff != 0).count()
}

/// What `runout verify` reports.
pub struct BitPerfect {
    pub rate: u32,
    pub channels: usize,
    pub bits: u16,
    pub seconds: f64,
    /// For a 24- or 32-bit file: samples with any of the low 8 bits set
    /// (None for a 16-bit file).
    pub low_bits_nonzero: Option<u64>,
    /// Runs of zeros or held samples: (start seconds, length ms).
    pub errors: Vec<(f64, f64)>,
}

/// Bit-perfect check of a digital capture. Reads in blocks.
pub fn verify_bit_perfect(path: &Path) -> Result<BitPerfect> {
    let a = decode_file(path)?;
    let bits = if a.bits <= 16 { 16 } else { 24 };
    let step = a.rate as usize * 10;
    let mut low = 0u64;
    let mut errors = Vec::new();
    let mut f = 0;
    while f < a.frames {
        let e = (f + step).min(a.frames);
        if bits > 16 {
            low += low_bits_nonzero(&a.read_int_interleaved(f, e, bits)) as u64;
        }
        // a little overlap so a run across a block edge is still seen whole
        let lead = 512.min(f);
        for c in 0..a.channels {
            let x = a.read_channel(c, (f - lead) as i64, e as i64);
            for ev in crate::dsp::dropout::find_errors(&x, a.rate, 4, 10.0) {
                if ev.pos >= lead {
                    errors.push(((f - lead + ev.pos) as f64 / a.rate as f64, ev.len as f64 * 1000.0 / a.rate as f64));
                }
            }
        }
        f = e;
    }
    errors.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
    errors.dedup_by(|x, y| (x.0 - y.0).abs() < 1e-3);
    Ok(BitPerfect { rate: a.rate, channels: a.channels, bits: a.bits, seconds: a.duration(), low_bits_nonzero: if bits > 16 { Some(low) } else { None }, errors })
}

/// Rewrite a 24-bit WAV whose low 8 bits are all zero as 16-bit (the raw of a
/// bit-perfect capture of a 16-bit source). Streams in blocks, so a long
/// capture is never held in memory. Returns true if it was rewritten.
pub fn shrink_to_16(path: &Path) -> Result<bool> {
    let a = decode_file(path)?;
    if a.bits <= 16 {
        return Ok(false);
    }
    let step = 1 << 16;
    let mut f = 0;
    while f < a.frames {
        let e = (f + step).min(a.frames);
        if low_bits_nonzero(&a.read_int_interleaved(f, e, 24)) > 0 {
            return Ok(false);
        }
        f = e;
    }
    let tmp = path.with_extension("wav.tmp16");
    {
        let mut w = WavWriter::create(&tmp, a.rate, a.channels as u16, 16)?;
        let mut f = 0;
        while f < a.frames {
            let e = (f + step).min(a.frames);
            let v: Vec<i32> = a.read_int_interleaved(f, e, 24).iter().map(|v| v >> 8).collect();
            w.write(&v)?;
            f = e;
        }
        w.finish()?;
    }
    drop(a);
    std::fs::rename(&tmp, path).map_err(|e| Error::Io(e.to_string()))?;
    Ok(true)
}

/// Fix the RIFF and data chunk sizes of a WAV whose writer died before
/// finalizing (a crash mid-capture). A capture that got past 4 GB is turned
/// into RF64 through its reserved JUNK chunk. Returns true if the header was
/// changed.
pub fn repair_wav_header(path: &Path) -> Result<bool> {
    use std::io::Read;
    let mut f = std::fs::OpenOptions::new().read(true).write(true).open(path).map_err(|e| Error::Io(e.to_string()))?;
    let len = f.metadata().map_err(|e| Error::Io(e.to_string()))?.len();
    {
        let mut h = [0u8; 48];
        if f.read_exact(&mut h).is_ok() && &h[0..4] == b"RIFF" && &h[12..16] == b"JUNK" && len > u32::MAX as u64 {
            if let Some(w) = wav_info(path) {
                let width = (w.bits as u64).div_ceil(8) * w.channels.max(1) as u64;
                let mut data = len - w.data_off;
                data -= data % width.max(1);
                upgrade_rf64(&mut f, data, data / width.max(1)).map_err(|e| Error::Io(e.to_string()))?;
                return Ok(true);
            }
        }
        f.seek(SeekFrom::Start(0)).map_err(|e| Error::Io(e.to_string()))?;
    }
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
            let data_len = data_len.min(u32::MAX as u64 - data_start);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_round_trips_through_symphonia_and_the_mapper() {
        let dir = std::env::temp_dir().join(format!("runout-wav-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("a.wav");
        let ints: Vec<i32> = (0..96000).map(|i| ((i as f64 * 0.01).sin() * 4_000_000.0) as i32).collect();
        let mut w = WavWriter::create(&p, 96000, 2, 24).unwrap();
        w.write(&ints).unwrap();
        assert_eq!(w.finish().unwrap(), 48000);
        let a = decode_symphonia(&p).unwrap();
        assert_eq!(a.read_int_interleaved(0, a.frames, 24), ints);
        let info = wav_info(&p).unwrap();
        let m = open_mapped(&p, &info).unwrap();
        assert_eq!(m.read_int_interleaved(0, m.frames, 24), ints);
        assert_eq!(m.sample(100, 1), a.sample(100, 1));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rf64_upgrade_reads_back() {
        // Build a small file and upgrade it by hand: same code path as past 4 GB.
        let dir = std::env::temp_dir().join(format!("runout-rf64-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("b.wav");
        let ints: Vec<i32> = (0..20000).map(|i| (i % 3000) - 1500).collect();
        let mut w = WavWriter::create(&p, 48000, 2, 16).unwrap();
        w.write(&ints).unwrap();
        w.flush_header().unwrap();
        let bytes = w.data_bytes;
        let frames = w.frames();
        drop(w);
        let mut f = std::fs::OpenOptions::new().read(true).write(true).open(&p).unwrap();
        upgrade_rf64(&mut f, bytes, frames).unwrap();
        drop(f);
        let info = wav_info(&p).unwrap();
        assert!(info.rf64);
        assert_eq!(info.data_len, bytes);
        let a = decode_file(&p).unwrap();
        assert_eq!(a.frames as u64, frames);
        assert_eq!(a.read_int_interleaved(0, a.frames, 16), ints);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
