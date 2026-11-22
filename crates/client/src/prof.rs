//! Frame profiler for the benchmark (`--poses ... --time`, or R5_PROF=1): CPU time and GPU timestamps at each mark in
//! `Gfx::draw`, so a pose's cost splits into its passes. Off (no queries, no cost) unless enabled at device creation.

use std::cell::{Cell, RefCell};
use std::time::Instant;

const MAX_MARKS: u32 = 32;

pub struct Prof {
    qs: wgpu::QuerySet,
    resolve: wgpu::Buffer,
    read: wgpu::Buffer,
    /// ns per timestamp tick
    period: f64,
    labels: RefCell<Vec<&'static str>>,
    cpu: RefCell<Vec<Instant>>,
    pending: Cell<bool>,
    /// label -> (cpu ms, gpu ms) summed over `frames`
    pub sums: RefCell<Vec<(&'static str, f64, f64)>>,
    pub frames: Cell<u32>,
}

/// The device features the profiler needs (requested only when R5_PROF is set or the benchmark runs).
pub fn features(adapter: &wgpu::Adapter) -> wgpu::Features {
    let want = wgpu::Features::TIMESTAMP_QUERY | wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS;
    if wanted() && adapter.features().contains(want) {
        want
    } else {
        wgpu::Features::empty()
    }
}

pub fn wanted() -> bool {
    std::env::var_os("R5_PROF").is_some_and(|v| v != "0")
}

impl Prof {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Option<Prof> {
        if !device.features().contains(wgpu::Features::TIMESTAMP_QUERY_INSIDE_ENCODERS) {
            return None;
        }
        let size = MAX_MARKS as u64 * 8;
        Some(Prof {
            qs: device.create_query_set(&wgpu::QuerySetDescriptor { label: Some("prof"), ty: wgpu::QueryType::Timestamp, count: MAX_MARKS }),
            resolve: device.create_buffer(&wgpu::BufferDescriptor { label: Some("prof resolve"), size, usage: wgpu::BufferUsages::QUERY_RESOLVE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false }),
            read: device.create_buffer(&wgpu::BufferDescriptor { label: Some("prof read"), size, usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST, mapped_at_creation: false }),
            period: queue.get_timestamp_period() as f64,
            labels: RefCell::new(Vec::new()),
            cpu: RefCell::new(Vec::new()),
            pending: Cell::new(false),
            sums: RefCell::new(Vec::new()),
            frames: Cell::new(0),
        })
    }

    /// Start a frame: the first mark.
    pub fn begin(&self, enc: &mut wgpu::CommandEncoder) {
        self.labels.borrow_mut().clear();
        self.cpu.borrow_mut().clear();
        self.mark(enc, "begin");
    }

    /// The time since the previous mark goes to `label`.
    pub fn mark(&self, enc: &mut wgpu::CommandEncoder, label: &'static str) {
        let mut l = self.labels.borrow_mut();
        if l.len() as u32 >= MAX_MARKS {
            return;
        }
        enc.write_timestamp(&self.qs, l.len() as u32);
        l.push(label);
        self.cpu.borrow_mut().push(Instant::now());
    }

    /// Last mark of the frame: copy the timestamps out (read back by `collect` after the submit).
    pub fn end(&self, enc: &mut wgpu::CommandEncoder) {
        self.mark(enc, "end");
        let n = self.labels.borrow().len() as u32;
        enc.resolve_query_set(&self.qs, 0..n, &self.resolve, 0);
        enc.copy_buffer_to_buffer(&self.resolve, 0, &self.read, 0, n as u64 * 8);
        self.pending.set(true);
    }

    /// After the frame's submit: wait for it, add its per-section times to `sums`.
    pub fn collect(&self, device: &wgpu::Device) {
        if !self.pending.replace(false) {
            return;
        }
        let labels = self.labels.borrow();
        let cpu = self.cpu.borrow();
        let n = labels.len();
        let slice = self.read.slice(..n as u64 * 8);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = device.poll(wgpu::PollType::wait_indefinitely());
        let ts: Vec<u64> = match slice.get_mapped_range() {
            Ok(v) => bytemuck::cast_slice(&v).to_vec(),
            Err(_) => return,
        };
        self.read.unmap();
        let mut sums = self.sums.borrow_mut();
        for i in 1..n {
            let gpu = ts[i].saturating_sub(ts[i - 1]) as f64 * self.period / 1e6;
            let c = (cpu[i] - cpu[i - 1]).as_secs_f64() * 1000.0;
            match sums.iter_mut().find(|s| s.0 == labels[i]) {
                Some(s) => {
                    s.1 += c;
                    s.2 += gpu;
                }
                None => sums.push((labels[i], c, gpu)),
            }
        }
        self.frames.set(self.frames.get() + 1);
    }

    /// "label cpu/gpu ms" per section, averaged, and clear.
    pub fn report(&self) -> String {
        let f = self.frames.replace(0).max(1) as f64;
        let mut sums = self.sums.borrow_mut();
        let (tc, tg) = sums.iter().fold((0.0, 0.0), |a, s| (a.0 + s.1, a.1 + s.2));
        let mut out = format!("cpu {:.2} / gpu {:.2} ms:", tc / f, tg / f);
        for s in sums.iter() {
            out += &format!(" {} {:.2}/{:.2}", s.0, s.1 / f, s.2 / f);
        }
        sums.clear();
        out
    }
}
