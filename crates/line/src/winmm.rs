#![cfg(all(target_os = "windows", target_arch = "x86"))]
#![allow(unsafe_code)]

use core::ffi::c_void;
use std::collections::VecDeque;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};

use dsp::Resampler;

pub const CHANNELS: usize = 2;

const MMSYSERR_NOERROR: u32 = 0;
const WAVE_MAPPER: u32 = u32::MAX;
const WAVE_FORMAT_PCM: u16 = 1;
const CALLBACK_EVENT: u32 = 0x0005_0000;
const WHDR_DONE: u32 = 0x0000_0001;
const INFINITE: u32 = u32::MAX;
const WAIT_OBJECT_0: u32 = 0;

type MMRESULT = u32;
type HWAVEOUT = *mut c_void;
type HWAVEIN = *mut c_void;
type HANDLE = *mut c_void;

#[repr(C)]
#[derive(Clone, Copy)]
struct WAVEFORMATEX {
    w_format_tag: u16,
    n_channels: u16,
    n_samples_per_sec: u32,
    n_avg_bytes_per_sec: u32,
    n_block_align: u16,
    w_bits_per_sample: u16,
    cb_size: u16,
}

#[repr(C)]
struct WAVEHDR {
    lp_data: *mut u8,
    dw_buffer_length: u32,
    dw_bytes_recorded: u32,
    dw_user: usize,
    dw_flags: u32,
    dw_loops: u32,
    lp_next: *mut WAVEHDR,
    reserved: usize,
}

#[repr(C)]
struct WAVEOUTCAPSW {
    w_mid: u16,
    w_pid: u16,
    v_driver_version: u32,
    sz_pname: [u16; 32],
    dw_formats: u32,
    w_channels: u16,
    w_reserved1: u16,
    dw_support: u32,
}

#[repr(C)]
struct WAVEINCAPSW {
    w_mid: u16,
    w_pid: u16,
    v_driver_version: u32,
    sz_pname: [u16; 32],
    dw_formats: u32,
    w_channels: u16,
    w_reserved1: u16,
}

#[link(name = "winmm")]
unsafe extern "system" {
    fn waveOutGetNumDevs() -> u32;
    fn waveOutGetDevCapsW(u_device_id: usize, pwoc: *mut WAVEOUTCAPSW, cbwoc: u32) -> MMRESULT;
    fn waveOutOpen(
        phwo: *mut HWAVEOUT,
        u_device_id: u32,
        pwfx: *const WAVEFORMATEX,
        dw_callback: usize,
        dw_instance: usize,
        fdw_open: u32,
    ) -> MMRESULT;
    fn waveOutPrepareHeader(hwo: HWAVEOUT, pwh: *mut WAVEHDR, cbwh: u32) -> MMRESULT;
    fn waveOutWrite(hwo: HWAVEOUT, pwh: *mut WAVEHDR, cbwh: u32) -> MMRESULT;
    fn waveOutUnprepareHeader(hwo: HWAVEOUT, pwh: *mut WAVEHDR, cbwh: u32) -> MMRESULT;
    fn waveOutReset(hwo: HWAVEOUT) -> MMRESULT;
    fn waveOutClose(hwo: HWAVEOUT) -> MMRESULT;

    fn waveInGetNumDevs() -> u32;
    fn waveInGetDevCapsW(u_device_id: usize, pwic: *mut WAVEINCAPSW, cbwic: u32) -> MMRESULT;
    fn waveInOpen(
        phwi: *mut HWAVEIN,
        u_device_id: u32,
        pwfx: *const WAVEFORMATEX,
        dw_callback: usize,
        dw_instance: usize,
        fdw_open: u32,
    ) -> MMRESULT;
    fn waveInPrepareHeader(hwi: HWAVEIN, pwh: *mut WAVEHDR, cbwh: u32) -> MMRESULT;
    fn waveInAddBuffer(hwi: HWAVEIN, pwh: *mut WAVEHDR, cbwh: u32) -> MMRESULT;
    fn waveInStart(hwi: HWAVEIN) -> MMRESULT;
    fn waveInUnprepareHeader(hwi: HWAVEIN, pwh: *mut WAVEHDR, cbwh: u32) -> MMRESULT;
    fn waveInReset(hwi: HWAVEIN) -> MMRESULT;
    fn waveInClose(hwi: HWAVEIN) -> MMRESULT;
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn CreateEventW(
        lp_event_attributes: *mut c_void,
        b_manual_reset: i32,
        b_initial_state: i32,
        lp_name: *const u16,
    ) -> HANDLE;
    fn SetEvent(h_event: HANDLE) -> i32;
    fn WaitForSingleObject(h_handle: HANDLE, dw_milliseconds: u32) -> u32;
    fn CloseHandle(h_object: HANDLE) -> i32;
}

struct EventHandle(HANDLE);
unsafe impl Send for EventHandle {}
unsafe impl Sync for EventHandle {}

impl EventHandle {
    fn new() -> Result<Self, String> {
        let h = unsafe { CreateEventW(ptr::null_mut(), 0, 0, ptr::null()) };
        if h.is_null() {
            Err("CreateEventW failed".to_owned())
        } else {
            Ok(Self(h))
        }
    }

    fn raw(&self) -> HANDLE {
        self.0
    }

    fn signal(&self) {
        unsafe {
            let _ = SetEvent(self.0);
        }
    }

    fn wait(&self, ms: u32) {
        unsafe {
            let _ = WaitForSingleObject(self.0, ms);
        }
    }
}

impl Drop for EventHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

fn wide_name(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    OsString::from_wide(&buf[..end]).to_string_lossy().into_owned()
}

fn pcm_format(channels: u16, sample_rate: u32) -> WAVEFORMATEX {
    let bits: u16 = 16;
    let block_align = channels * (bits / 8);
    WAVEFORMATEX {
        w_format_tag: WAVE_FORMAT_PCM,
        n_channels: channels,
        n_samples_per_sec: sample_rate,
        n_avg_bytes_per_sec: sample_rate * u32::from(block_align),
        n_block_align: block_align,
        w_bits_per_sample: bits,
        cb_size: 0,
    }
}

fn f32_to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * f32::from(i16::MAX)) as i16
}

fn i16_to_f32(sample: i16) -> f32 {
    f32::from(sample) / 32768.0
}

fn find_out_id(want: Option<&str>) -> Result<(u32, String, u16), String> {
    let count = unsafe { waveOutGetNumDevs() };
    let mut list = Vec::new();
    for id in 0..count {
        let mut caps = unsafe { std::mem::zeroed::<WAVEOUTCAPSW>() };
        if unsafe { waveOutGetDevCapsW(id as usize, &mut caps, std::mem::size_of::<WAVEOUTCAPSW>() as u32) }
            == MMSYSERR_NOERROR
        {
            list.push((id, wide_name(&caps.sz_pname), caps.w_channels.max(1)));
        }
    }
    match want {
        None => {
            if let Some(first) = list.into_iter().next() {
                Ok(first)
            } else {
                Ok((WAVE_MAPPER, "Default WaveOut".to_owned(), 2))
            }
        }
        Some(name) => {
            let lower = name.to_lowercase();
            list.into_iter()
                .find(|(_, n, _)| n.eq_ignore_ascii_case(name))
                .or_else(|| {
                    let all: Vec<(u32, String, u16)> = (0..count)
                        .filter_map(|id| {
                            let mut caps = unsafe { std::mem::zeroed::<WAVEOUTCAPSW>() };
                            (unsafe {
                                waveOutGetDevCapsW(
                                    id as usize,
                                    &mut caps,
                                    std::mem::size_of::<WAVEOUTCAPSW>() as u32,
                                )
                            } == MMSYSERR_NOERROR)
                                .then(|| (id, wide_name(&caps.sz_pname), caps.w_channels.max(1)))
                        })
                        .collect();
                    all.into_iter()
                        .find(|(_, n, _)| n.to_lowercase().contains(&lower))
                })
                .ok_or_else(|| format!("no output device named {name:?}"))
        }
    }
}

fn find_in_id(want: Option<&str>) -> Result<(u32, String, u16), String> {
    let count = unsafe { waveInGetNumDevs() };
    let mut list = Vec::new();
    for id in 0..count {
        let mut caps = unsafe { std::mem::zeroed::<WAVEINCAPSW>() };
        if unsafe { waveInGetDevCapsW(id as usize, &mut caps, std::mem::size_of::<WAVEINCAPSW>() as u32) }
            == MMSYSERR_NOERROR
        {
            list.push((id, wide_name(&caps.sz_pname), caps.w_channels.max(1)));
        }
    }
    match want {
        None => {
            if let Some(first) = list.into_iter().next() {
                Ok(first)
            } else {
                Ok((WAVE_MAPPER, "Default WaveIn".to_owned(), 1))
            }
        }
        Some(name) => {
            let lower = name.to_lowercase();
            list.into_iter()
                .find(|(_, n, _)| n.eq_ignore_ascii_case(name) || n.to_lowercase().contains(&lower))
                .ok_or_else(|| format!("no input device named {name:?}"))
        }
    }
}

fn open_wave_out(dev_id: u32, channels: u16, preferred_rate: u32) -> Result<(HWAVEOUT, u32, Arc<EventHandle>), String> {
    let event = Arc::new(EventHandle::new()?);
    for &rate in &[preferred_rate, 48_000, 44_100, 16_000, 8_000] {
        let fmt = pcm_format(channels, rate);
        let mut handle: HWAVEOUT = ptr::null_mut();
        let res = unsafe {
            waveOutOpen(
                &mut handle,
                dev_id,
                &fmt,
                event.raw() as usize,
                0,
                CALLBACK_EVENT,
            )
        };
        if res == MMSYSERR_NOERROR && !handle.is_null() {
            return Ok((handle, rate, event));
        }
    }
    Err("waveOutOpen failed for supported sample rates".to_owned())
}

fn open_wave_in(dev_id: u32, channels: u16, preferred_rate: u32) -> Result<(HWAVEIN, u32, Arc<EventHandle>), String> {
    let event = Arc::new(EventHandle::new()?);
    for &rate in &[preferred_rate, 48_000, 44_100, 16_000, 8_000] {
        let fmt = pcm_format(channels, rate);
        let mut handle: HWAVEIN = ptr::null_mut();
        let res = unsafe {
            waveInOpen(
                &mut handle,
                dev_id,
                &fmt,
                event.raw() as usize,
                0,
                CALLBACK_EVENT,
            )
        };
        if res == MMSYSERR_NOERROR && !handle.is_null() {
            return Ok((handle, rate, event));
        }
    }
    Err("waveInOpen failed for supported sample rates".to_owned())
}

struct WaveOutWorker {
    stop: Arc<AtomicBool>,
    event: Arc<EventHandle>,
    join: Option<JoinHandle<()>>,
}

impl WaveOutWorker {
    fn start<F>(
        handle: HWAVEOUT,
        event: Arc<EventHandle>,
        channels: usize,
        frames_per_buf: usize,
        mut fill: F,
    ) -> Self
    where
        F: FnMut(&mut [f32]) + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let event_thread = Arc::clone(&event);
        let handle_addr = handle as usize;

        let join = thread::spawn(move || {
            let hwo = handle_addr as HWAVEOUT;
            const NUM_BUFS: usize = 4;
            let samples_per_buf = frames_per_buf * channels;
            let bytes_per_buf = samples_per_buf * std::mem::size_of::<i16>();

            let mut pcm_bufs: Vec<Vec<i16>> = (0..NUM_BUFS).map(|_| vec![0i16; samples_per_buf]).collect();
            let mut hdrs: Vec<Box<WAVEHDR>> = pcm_bufs
                .iter_mut()
                .map(|buf| {
                    Box::new(WAVEHDR {
                        lp_data: buf.as_mut_ptr() as *mut u8,
                        dw_buffer_length: bytes_per_buf as u32,
                        dw_bytes_recorded: 0,
                        dw_user: 0,
                        dw_flags: 0,
                        dw_loops: 0,
                        lp_next: ptr::null_mut(),
                        reserved: 0,
                    })
                })
                .collect();

            let mut f32_scratch = vec![0.0f32; samples_per_buf];

            for i in 0..NUM_BUFS {
                f32_scratch.fill(0.0);
                fill(&mut f32_scratch);
                for (dst, &src) in pcm_bufs[i].iter_mut().zip(f32_scratch.iter()) {
                    *dst = f32_to_i16(src);
                }
                unsafe {
                    let pwh: *mut WAVEHDR = &mut *hdrs[i];
                    let _ = waveOutPrepareHeader(hwo, pwh, std::mem::size_of::<WAVEHDR>() as u32);
                    let _ = waveOutWrite(hwo, pwh, std::mem::size_of::<WAVEHDR>() as u32);
                }
            }

            while !stop_thread.load(Ordering::Relaxed) {
                event_thread.wait(50);
                if stop_thread.load(Ordering::Relaxed) {
                    break;
                }
                for i in 0..NUM_BUFS {
                    let flags = unsafe { std::ptr::read_volatile(&hdrs[i].dw_flags) };
                    if (flags & WHDR_DONE) != 0 {
                        f32_scratch.fill(0.0);
                        fill(&mut f32_scratch);
                        for (dst, &src) in pcm_bufs[i].iter_mut().zip(f32_scratch.iter()) {
                            *dst = f32_to_i16(src);
                        }
                        unsafe {
                            let pwh: *mut WAVEHDR = &mut *hdrs[i];
                            let _ = waveOutWrite(hwo, pwh, std::mem::size_of::<WAVEHDR>() as u32);
                        }
                    }
                }
            }

            unsafe {
                let _ = waveOutReset(hwo);
                for i in 0..NUM_BUFS {
                    let pwh: *mut WAVEHDR = &mut *hdrs[i];
                    let _ = waveOutUnprepareHeader(hwo, pwh, std::mem::size_of::<WAVEHDR>() as u32);
                }
                let _ = waveOutClose(hwo);
            }
        });

        Self {
            stop,
            event,
            join: Some(join),
        }
    }
}

impl Drop for WaveOutWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.event.signal();
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

struct WaveInWorker {
    stop: Arc<AtomicBool>,
    event: Arc<EventHandle>,
    join: Option<JoinHandle<()>>,
}

impl WaveInWorker {
    fn start<F>(
        handle: HWAVEIN,
        event: Arc<EventHandle>,
        channels: usize,
        frames_per_buf: usize,
        mut on_samples: F,
    ) -> Self
    where
        F: FnMut(&[f32]) + Send + 'static,
    {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let event_thread = Arc::clone(&event);
        let handle_addr = handle as usize;

        let join = thread::spawn(move || {
            let hwi = handle_addr as HWAVEIN;
            const NUM_BUFS: usize = 4;
            let samples_per_buf = frames_per_buf * channels;
            let bytes_per_buf = samples_per_buf * std::mem::size_of::<i16>();

            let mut pcm_bufs: Vec<Vec<i16>> = (0..NUM_BUFS).map(|_| vec![0i16; samples_per_buf]).collect();
            let mut hdrs: Vec<Box<WAVEHDR>> = pcm_bufs
                .iter_mut()
                .map(|buf| {
                    Box::new(WAVEHDR {
                        lp_data: buf.as_mut_ptr() as *mut u8,
                        dw_buffer_length: bytes_per_buf as u32,
                        dw_bytes_recorded: 0,
                        dw_user: 0,
                        dw_flags: 0,
                        dw_loops: 0,
                        lp_next: ptr::null_mut(),
                        reserved: 0,
                    })
                })
                .collect();

            let mut f32_scratch = vec![0.0f32; samples_per_buf];

            for i in 0..NUM_BUFS {
                unsafe {
                    let pwh: *mut WAVEHDR = &mut *hdrs[i];
                    let _ = waveInPrepareHeader(hwi, pwh, std::mem::size_of::<WAVEHDR>() as u32);
                    let _ = waveInAddBuffer(hwi, pwh, std::mem::size_of::<WAVEHDR>() as u32);
                }
            }
            unsafe {
                let _ = waveInStart(hwi);
            }

            while !stop_thread.load(Ordering::Relaxed) {
                event_thread.wait(50);
                if stop_thread.load(Ordering::Relaxed) {
                    break;
                }
                for i in 0..NUM_BUFS {
                    let flags = unsafe { std::ptr::read_volatile(&hdrs[i].dw_flags) };
                    if (flags & WHDR_DONE) != 0 {
                        let recorded = unsafe { std::ptr::read_volatile(&hdrs[i].dw_bytes_recorded) } as usize
                            / std::mem::size_of::<i16>();
                        let n = recorded.min(pcm_bufs[i].len());
                        f32_scratch.clear();
                        f32_scratch.extend(pcm_bufs[i][..n].iter().map(|&s| i16_to_f32(s)));
                        on_samples(&f32_scratch);
                        unsafe {
                            let pwh: *mut WAVEHDR = &mut *hdrs[i];
                            let _ = waveInAddBuffer(hwi, pwh, std::mem::size_of::<WAVEHDR>() as u32);
                        }
                    }
                }
            }

            unsafe {
                let _ = waveInReset(hwi);
                for i in 0..NUM_BUFS {
                    let pwh: *mut WAVEHDR = &mut *hdrs[i];
                    let _ = waveInUnprepareHeader(hwi, pwh, std::mem::size_of::<WAVEHDR>() as u32);
                }
                let _ = waveInClose(hwi);
            }
        });

        Self {
            stop,
            event,
            join: Some(join),
        }
    }
}

impl Drop for WaveInWorker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.event.signal();
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

// ---------------- audio.rs equivalents ----------------

#[derive(Debug)]
pub struct AudioSink {
    buf: Mutex<VecDeque<f32>>,
    enabled: AtomicBool,
    capacity: usize,
    overruns: AtomicU64,
    underruns: AtomicU64,
    volume: AtomicU32,
    primed: AtomicBool,
}

impl AudioSink {
    pub fn new(capacity: usize) -> Self {
        Self {
            buf: Mutex::new(VecDeque::with_capacity(capacity)),
            enabled: AtomicBool::new(false),
            capacity,
            overruns: AtomicU64::new(0),
            underruns: AtomicU64::new(0),
            volume: AtomicU32::new(0.5f32.to_bits()),
            primed: AtomicBool::new(false),
        }
    }

    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }

    pub fn set_volume(&self, volume: f32) {
        self.volume.store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
        if !on {
            self.primed.store(false, Ordering::Relaxed);
            if let Ok(mut b) = self.buf.lock() {
                b.clear();
            }
        }
    }

    pub fn push(&self, samples: &[f32]) {
        if !self.enabled() {
            return;
        }
        if let Ok(mut b) = self.buf.lock() {
            for &s in samples {
                if b.len() >= self.capacity {
                    b.pop_front();
                    self.overruns.fetch_add(1, Ordering::Relaxed);
                }
                b.push_back(s);
            }
        }
    }

    fn drain(&self, out: &mut [f32]) -> usize {
        match self.buf.try_lock() {
            Ok(mut b) => {
                if !self.primed.load(Ordering::Relaxed) {
                    if b.len() < self.capacity / 3 {
                        return 0;
                    }
                    self.primed.store(true, Ordering::Relaxed);
                }
                let n = out.len().min(b.len());
                for slot in out.iter_mut().take(n) {
                    *slot = b.pop_front().unwrap_or(0.0);
                }
                if n < out.len() {
                    self.underruns.fetch_add(1, Ordering::Relaxed);
                    self.primed.store(false, Ordering::Relaxed);
                }
                n
            }
            Err(_) => 0,
        }
    }

    pub fn overruns(&self) -> u64 {
        self.overruns.load(Ordering::Relaxed)
    }

    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }
}

pub fn output_devices() -> Vec<String> {
    let count = unsafe { waveOutGetNumDevs() };
    let mut names = Vec::new();
    for id in 0..count {
        let mut caps = unsafe { std::mem::zeroed::<WAVEOUTCAPSW>() };
        if unsafe { waveOutGetDevCapsW(id as usize, &mut caps, std::mem::size_of::<WAVEOUTCAPSW>() as u32) }
            == MMSYSERR_NOERROR
        {
            names.push(wide_name(&caps.sz_pname));
        }
    }
    names
}

pub struct Monitor {
    _worker: WaveOutWorker,
    pub device: String,
    pub sample_rate: u32,
    pub channels: u16,
}

impl std::fmt::Debug for Monitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Monitor")
            .field("device", &self.device)
            .field("sample_rate", &self.sample_rate)
            .field("channels", &self.channels)
            .finish_non_exhaustive()
    }
}

impl Monitor {
    pub fn play(&self) -> Result<(), String> {
        Ok(())
    }
}

pub fn listen(
    sink: Arc<AudioSink>,
    device_name: Option<&str>,
    source_rate: f64,
) -> Result<Monitor, String> {
    let (dev_id, name, dev_ch) = find_out_id(device_name)?;
    let channels = dev_ch.min(2).max(1);
    let (handle, sample_rate, event) = open_wave_out(dev_id, channels, 48_000)?;
    let step = source_rate / sample_rate as f64;

    let mut pending: Vec<f32> = Vec::new();
    let mut pos = 0.0f64;

    let worker = WaveOutWorker::start(handle, event, channels as usize, 1024, move |out: &mut [f32]| {
        let frames = out.len() / channels as usize;
        let needed = (pos + step * frames as f64).ceil() as usize + 2;
        if pending.len() < needed {
            let mut extra = vec![0.0f32; needed - pending.len()];
            let got = sink.drain(&mut extra);
            extra.truncate(got);
            pending.extend_from_slice(&extra);
        }
        let volume = sink.volume();
        for frame in out.chunks_mut(channels as usize) {
            let i = pos as usize;
            let sample = if i + 1 < pending.len() {
                let frac = (pos - i as f64) as f32;
                pending[i] * (1.0 - frac) + pending[i + 1] * frac
            } else {
                0.0
            };
            for slot in frame.iter_mut() {
                *slot = sample * volume;
            }
            pos += step;
        }
        let consumed = pos as usize;
        if consumed > 0 && consumed <= pending.len() {
            pending.drain(..consumed);
            pos -= consumed as f64;
        } else if consumed > pending.len() {
            pending.clear();
            pos = 0.0;
        }
    });

    Ok(Monitor {
        _worker: worker,
        device: name,
        sample_rate,
        channels,
    })
}

// ---------------- duplex.rs equivalents ----------------

const DUPLEX_BUFFER: usize = 4096;
const DUPLEX_PRIME: usize = 1024;
const DUPLEX_RESERVE: usize = 2048;
const DUPLEX_SETTLE: usize = 4000;

#[derive(Debug, Default)]
struct DuplexCounters {
    dropped_in: AtomicU64,
    dropped_out: AtomicU64,
    underruns: AtomicU64,
    contended: AtomicU64,
}

#[derive(Debug)]
struct DuplexShared {
    incoming: Mutex<VecDeque<f32>>,
    settling: Mutex<usize>,
    delivering: AtomicBool,
    outgoing: Mutex<VecDeque<f32>>,
    counters: DuplexCounters,
}

pub fn input_devices() -> Vec<String> {
    let count = unsafe { waveInGetNumDevs() };
    let mut names = Vec::new();
    for id in 0..count {
        let mut caps = unsafe { std::mem::zeroed::<WAVEINCAPSW>() };
        if unsafe { waveInGetDevCapsW(id as usize, &mut caps, std::mem::size_of::<WAVEINCAPSW>() as u32) }
            == MMSYSERR_NOERROR
        {
            names.push(wide_name(&caps.sz_pname));
        }
    }
    names
}

pub struct Duplex {
    _input: WaveInWorker,
    _output: WaveOutWorker,
    shared: Arc<DuplexShared>,
    pub input_device: String,
    pub output_device: String,
    pub input_rate: u32,
    pub output_rate: u32,
}

impl std::fmt::Debug for Duplex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Duplex")
            .field("input_device", &self.input_device)
            .field("output_device", &self.output_device)
            .field("input_rate", &self.input_rate)
            .field("output_rate", &self.output_rate)
            .finish_non_exhaustive()
    }
}

impl Duplex {
    pub fn open(
        input: Option<&str>,
        output: Option<&str>,
        modem_rate: f64,
    ) -> Result<Self, String> {
        let (in_id, in_name, in_ch) = find_in_id(input)?;
        let (out_id, out_name, out_ch) = find_out_id(output)?;
        let in_channels = in_ch.min(2).max(1) as usize;
        let out_channels = out_ch.min(2).max(1) as usize;

        let (in_handle, in_rate, in_event) = open_wave_in(in_id, in_channels as u16, 48_000)?;
        let (out_handle, out_rate, out_event) = open_wave_out(out_id, out_channels as u16, 48_000)?;

        let shared = Arc::new(DuplexShared {
            incoming: Mutex::new(VecDeque::with_capacity(DUPLEX_BUFFER)),
            settling: Mutex::new(DUPLEX_SETTLE),
            delivering: AtomicBool::new(false),
            outgoing: Mutex::new(VecDeque::from(vec![0.0; DUPLEX_PRIME])),
            counters: DuplexCounters::default(),
        });

        let capture = Arc::clone(&shared);
        let mut down = Resampler::new(f64::from(in_rate), modem_rate);
        let mut converted: Vec<f64> = Vec::with_capacity(1024);
        let input_worker = WaveInWorker::start(in_handle, in_event, in_channels, 512, move |data: &[f32]| {
            converted.clear();
            for frame in data.chunks(in_channels) {
                let mono = frame.iter().sum::<f32>() / in_channels as f32;
                down.process(f64::from(mono), &mut converted);
            }
            let Ok(mut queue) = capture.incoming.try_lock() else {
                capture.counters.contended.fetch_add(1, Ordering::Relaxed);
                return;
            };
            let mut left = capture.settling.try_lock().ok();
            for &s in converted.iter() {
                if let Some(n) = left.as_deref_mut()
                    && *n > 0
                {
                    *n -= 1;
                    if *n == 0 {
                        capture.delivering.store(true, Ordering::Relaxed);
                    }
                    continue;
                }
                if queue.len() >= DUPLEX_BUFFER {
                    queue.pop_front();
                    capture.counters.dropped_in.fetch_add(1, Ordering::Relaxed);
                }
                queue.push_back(s as f32);
            }
        });

        let playback = Arc::clone(&shared);
        let mut up = Resampler::new(modem_rate, f64::from(out_rate));
        let mut ready: VecDeque<f64> = VecDeque::with_capacity(1024);
        let mut produced: Vec<f64> = Vec::with_capacity(64);
        let output_worker = WaveOutWorker::start(out_handle, out_event, out_channels, 512, move |out: &mut [f32]| {
            let frames = out.len() / out_channels;
            if !playback.delivering.load(Ordering::Relaxed) {
                out.fill(0.0);
                return;
            }
            match playback.outgoing.try_lock() {
                Ok(mut queue) => {
                    while ready.len() < frames + DUPLEX_RESERVE {
                        let Some(next) = queue.pop_front() else { break };
                        produced.clear();
                        up.process(f64::from(next), &mut produced);
                        ready.extend(produced.iter().copied());
                    }
                }
                Err(_) => {
                    playback.counters.contended.fetch_add(1, Ordering::Relaxed);
                }
            }
            if ready.len() < frames {
                playback.counters.underruns.fetch_add(1, Ordering::Relaxed);
            }
            for frame in out.chunks_mut(out_channels) {
                let s = ready.pop_front().unwrap_or(0.0) as f32;
                for slot in frame.iter_mut() {
                    *slot = s;
                }
            }
        });

        Ok(Self {
            _input: input_worker,
            _output: output_worker,
            shared,
            input_device: in_name,
            output_device: out_name,
            input_rate: in_rate,
            output_rate: out_rate,
        })
    }

    pub fn settled(&self) -> bool {
        self.shared.settling.lock().map(|n| *n == 0).unwrap_or(false)
    }

    pub fn receive(&self, into: &mut Vec<f32>) {
        let Ok(mut queue) = self.shared.incoming.lock() else {
            return;
        };
        into.extend(queue.drain(..));
    }

    pub fn transmit(&self, samples: &[f32]) {
        let Ok(mut queue) = self.shared.outgoing.lock() else {
            return;
        };
        for &s in samples {
            if queue.len() >= DUPLEX_BUFFER {
                self.shared.counters.dropped_out.fetch_add(1, Ordering::Relaxed);
                queue.pop_front();
            }
            queue.push_back(s);
        }
    }

    pub fn pending(&self) -> usize {
        self.shared.outgoing.lock().map(|q| q.len()).unwrap_or(0)
    }

    pub fn dropped_in(&self) -> u64 {
        self.shared.counters.dropped_in.load(Ordering::Relaxed)
    }

    pub fn dropped_out(&self) -> u64 {
        self.shared.counters.dropped_out.load(Ordering::Relaxed)
    }

    pub fn underruns(&self) -> u64 {
        self.shared.counters.underruns.load(Ordering::Relaxed)
    }

    pub fn contended(&self) -> u64 {
        self.shared.counters.contended.load(Ordering::Relaxed)
    }
}

// ---------------- wide.rs equivalents ----------------

const WIDE_BUFFER: usize = 96_000;
const WIDE_PRIME_PER_SECOND: usize = 25;
const WIDE_SETTLE: usize = 12_000;

#[derive(Debug, Default)]
struct WideCounters {
    dropped_in: AtomicU64,
    dropped_out: AtomicU64,
    underruns: AtomicU64,
}

#[derive(Debug)]
struct WideShared {
    incoming: Mutex<VecDeque<f32>>,
    settling: Mutex<usize>,
    delivering: AtomicBool,
    outgoing: Mutex<VecDeque<f32>>,
    counters: WideCounters,
}

pub struct Wide {
    _input: WaveInWorker,
    _output: WaveOutWorker,
    shared: Arc<WideShared>,
    pub input_device: String,
    pub output_device: String,
    pub rate: u32,
    pub output_rate: u32,
    pub input_channels: usize,
    pub output_channels: usize,
}

impl std::fmt::Debug for Wide {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wide")
            .field("input_device", &self.input_device)
            .field("output_device", &self.output_device)
            .field("rate", &self.rate)
            .finish_non_exhaustive()
    }
}

impl Wide {
    pub fn open(input: Option<&str>, output: Option<&str>) -> Result<Self, String> {
        let (in_id, in_name, in_ch) = find_in_id(input)?;
        let (out_id, out_name, out_ch) = find_out_id(output)?;
        let in_channels = in_ch.min(2).max(1) as usize;
        let out_channels = out_ch.min(2).max(1) as usize;

        let (in_handle, rate, in_event) = open_wave_in(in_id, in_channels as u16, 48_000)?;
        let (out_handle, output_rate, out_event) = open_wave_out(out_id, out_channels as u16, 48_000)?;

        let shared = Arc::new(WideShared {
            incoming: Mutex::new(VecDeque::with_capacity(WIDE_BUFFER * CHANNELS)),
            settling: Mutex::new(WIDE_SETTLE),
            delivering: AtomicBool::new(false),
            outgoing: Mutex::new(VecDeque::from(vec![
                0.0;
                output_rate as usize / WIDE_PRIME_PER_SECOND * CHANNELS
            ])),
            counters: WideCounters::default(),
        });

        let capture = Arc::clone(&shared);
        let input_worker = WaveInWorker::start(in_handle, in_event, in_channels, 512, move |data: &[f32]| {
            let Ok(mut queue) = capture.incoming.lock() else { return };
            let Ok(mut left) = capture.settling.lock() else { return };
            for frame in data.chunks(in_channels) {
                if *left > 0 {
                    *left -= 1;
                    if *left == 0 {
                        capture.delivering.store(true, Ordering::Relaxed);
                    }
                    continue;
                }
                if queue.len() >= WIDE_BUFFER * CHANNELS {
                    queue.drain(..CHANNELS);
                    capture.counters.dropped_in.fetch_add(1, Ordering::Relaxed);
                }
                queue.push_back(frame[0]);
                queue.push_back(frame[if in_channels > 1 { 1 } else { 0 }]);
            }
        });

        let playback = Arc::clone(&shared);
        let output_worker = WaveOutWorker::start(out_handle, out_event, out_channels, 512, move |out: &mut [f32]| {
            if !playback.delivering.load(Ordering::Relaxed) {
                out.fill(0.0);
                return;
            }
            let Ok(mut queue) = playback.outgoing.lock() else {
                out.fill(0.0);
                return;
            };
            if queue.len() < out.len() / out_channels * CHANNELS {
                playback.counters.underruns.fetch_add(1, Ordering::Relaxed);
            }
            for frame in out.chunks_mut(out_channels) {
                let l = queue.pop_front().unwrap_or(0.0);
                let r = queue.pop_front().unwrap_or(0.0);
                frame.fill(0.0);
                frame[0] = l;
                if out_channels > 1 {
                    frame[1] = r;
                }
            }
        });

        Ok(Self {
            _input: input_worker,
            _output: output_worker,
            shared,
            input_device: in_name,
            output_device: out_name,
            rate,
            output_rate,
            input_channels: in_channels,
            output_channels: out_channels,
        })
    }

    pub fn receive(&self, into: &mut Vec<f32>) {
        if let Ok(mut queue) = self.shared.incoming.lock() {
            into.extend(queue.drain(..));
        }
    }

    pub fn transmit(&self, frames: &[f32]) {
        let Ok(mut queue) = self.shared.outgoing.lock() else { return };
        for frame in frames.as_chunks::<CHANNELS>().0 {
            if queue.len() >= WIDE_BUFFER * CHANNELS {
                queue.drain(..CHANNELS);
                self.shared.counters.dropped_out.fetch_add(1, Ordering::Relaxed);
            }
            queue.extend(frame);
        }
    }

    pub fn wanted(&self) -> usize {
        (self.output_rate as usize / 16).saturating_sub(self.pending())
    }

    pub fn pending(&self) -> usize {
        self.shared.outgoing.lock().map(|q| q.len() / CHANNELS).unwrap_or(0)
    }

    pub fn dropped_in(&self) -> u64 {
        self.shared.counters.dropped_in.load(Ordering::Relaxed)
    }

    pub fn dropped_out(&self) -> u64 {
        self.shared.counters.dropped_out.load(Ordering::Relaxed)
    }

    pub fn underruns(&self) -> u64 {
        self.shared.counters.underruns.load(Ordering::Relaxed)
    }
}
