//! A texture of its own for each playing video (`native_video_open`,
//! docs/native-libraries.md), filled without the window's thread touching
//! the pixels.
//!
//! A 4K frame is 33 MiB: staged on the window's thread it cost 3.5 ms a
//! frame. Here a thread of its own (`video-upload`) copies the frame into a
//! mapped staging buffer, and the window's thread only unmaps it and records
//! one buffer-to-texture copy in the frame's encoder ([`VideoTextures::prepare`]),
//! ahead of the pass that samples the texture: one texture a slot is enough,
//! and nothing tears. The buffer is mapped again once that frame is
//! submitted (`map_buffer_on_submit`), so two of them a slot keep the copying
//! thread busy while one is on the GPU.
//!
//! A closed slot holds no GPU memory, and the thread ends with the last one.
//!
//! A staging buffer is never dropped while the state is locked: dropping one
//! that waits to be mapped runs its callback at once, which locks the state.

use crate::video_slots::{VIDEO_HEIGHT, VIDEO_SLOTS, VIDEO_WIDTH};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::Instant;

/// Takes a frame's pixels back once they are copied (the player renders into
/// them again), on whichever thread is done with them.
pub type Recycle = Box<dyn FnOnce(Vec<u8>) + Send>;

/// A decoded frame: tightly packed RGBA8, `width` by `height`.
pub struct VideoFrame {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    pub recycle: Recycle,
}

impl VideoFrame {
    fn give_back(self) {
        (self.recycle)(self.rgba);
    }

    /// No larger than a video slot, with as many bytes as its size says.
    fn valid(&self) -> bool {
        (1..=VIDEO_WIDTH).contains(&self.width)
            && (1..=VIDEO_HEIGHT).contains(&self.height)
            && self.rgba.len() == self.width as usize * self.height as usize * 4
    }
}

/// The staging buffers a slot keeps: one can be on the GPU while the other is written.
const STAGING: usize = 2;

/// Where a staging buffer is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Stage {
    /// Mapped and free: the copying thread may take it.
    Mapped,
    /// The copying thread is writing a frame into it.
    Writing,
    /// Holds a frame the window's thread has not taken yet; still mapped.
    Ready,
    /// Unmapped, in a frame's commands; mapped again once they are submitted.
    InFlight,
}

struct Staging {
    id: u64,
    buffer: wgpu::Buffer,
    size: (u32, u32),
    stage: Stage,
    /// Which frame it holds, counted up: the newest `Ready` one is shown.
    frame: u64,
}

#[derive(Default)]
struct Slot {
    /// The newest frame not copied yet.
    pending: Option<VideoFrame>,
    staging: Vec<Staging>,
    /// Counts the slot's closings: a buffer made for an earlier video is dropped.
    epoch: u64,
}

#[derive(Default)]
struct State {
    slots: [Slot; VIDEO_SLOTS],
    next_id: u64,
    frames: u64,
    /// The copying thread is running.
    running: bool,
    stop: bool,
    worker: Option<std::thread::JoinHandle<()>>,
}

struct Shared {
    device: wgpu::Device,
    state: Mutex<State>,
    wake: Condvar,
}

/// What the copying thread does for a slot with a frame of `size` waiting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Step {
    /// Copy the frame into the staging buffer at this index.
    Copy(usize),
    /// Make a staging buffer of the frame's size.
    Create,
    /// Every buffer is on the GPU: the frame waits (a newer one replaces it).
    Wait,
}

/// A free buffer first; then a new one while the slot has fewer than
/// [`STAGING`]; then over a frame that was never taken, which the waiting one
/// is newer than. Buffers of another size do not count.
pub(crate) fn next_step(
    size: (u32, u32),
    staging: impl Iterator<Item = ((u32, u32), Stage)> + Clone,
) -> Step {
    let fitting = staging.clone().filter(|&(s, _)| s == size).count();
    let at = |stage| staging.clone().position(|(s, at)| s == size && at == stage);
    if let Some(i) = at(Stage::Mapped) {
        Step::Copy(i)
    } else if fitting < STAGING {
        Step::Create
    } else if let Some(i) = at(Stage::Ready) {
        Step::Copy(i)
    } else {
        Step::Wait
    }
}

/// Bytes a row of `width` RGBA8 pixels takes in a buffer copied to a texture.
pub(crate) fn padded_row(width: u32) -> u32 {
    (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT)
}

enum Job {
    Copy {
        slot: usize,
        id: u64,
        buffer: wgpu::Buffer,
        frame: VideoFrame,
    },
    Create {
        slot: usize,
        epoch: u64,
        size: (u32, u32),
    },
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The next thing for the copying thread to do, if any. Buffers it has
    /// no more use for go to `retired`, to drop once the state is unlocked.
    fn job(state: &mut State, retired: &mut Vec<Staging>) -> Option<Job> {
        for (index, slot) in state.slots.iter_mut().enumerate() {
            let Some(frame) = &slot.pending else {
                continue;
            };
            let size = (frame.width, frame.height);
            // Another file's size: its buffers are of no use. One on the GPU
            // goes when it comes back (`remapped`).
            let (kept, gone) = std::mem::take(&mut slot.staging)
                .into_iter()
                .partition(|s| s.size == size || s.stage == Stage::InFlight);
            slot.staging = kept;
            retired.extend::<Vec<Staging>>(gone);
            match next_step(size, slot.staging.iter().map(|s| (s.size, s.stage))) {
                Step::Copy(i) => {
                    let staging = &mut slot.staging[i];
                    staging.stage = Stage::Writing;
                    return Some(Job::Copy {
                        slot: index,
                        id: staging.id,
                        buffer: staging.buffer.clone(),
                        frame: slot.pending.take()?,
                    });
                }
                Step::Create => {
                    return Some(Job::Create {
                        slot: index,
                        epoch: slot.epoch,
                        size,
                    });
                }
                Step::Wait => {}
            }
        }
        None
    }

    /// The copying thread: until the last slot is closed, or the textures are dropped.
    fn run(self: Arc<Self>) {
        let mut state = self.lock();
        let mut retired = Vec::new();
        loop {
            if state.stop {
                break;
            }
            let job = Self::job(&mut state, &mut retired);
            if !retired.is_empty() {
                drop(state);
                retired.clear();
                state = self.lock();
                // What was unlocked may have changed: look again, unless a
                // frame is already taken for copying.
                if job.is_none() {
                    continue;
                }
            }
            match job {
                Some(Job::Copy {
                    slot,
                    id,
                    buffer,
                    frame,
                }) => {
                    drop(state);
                    let copied = copy(&buffer, &frame);
                    frame.give_back();
                    state = self.lock();
                    state.frames += 1;
                    let number = state.frames;
                    let staging = &mut state.slots[slot].staging;
                    // Not there: its video was closed while it was written.
                    if let Some(i) = staging.iter().position(|s| s.id == id) {
                        if copied {
                            staging[i].stage = Stage::Ready;
                            staging[i].frame = number;
                        } else {
                            retired.push(staging.remove(i));
                        }
                    }
                    drop(state);
                    // The last use of a closed video's buffer may be this one.
                    drop(buffer);
                    retired.clear();
                    state = self.lock();
                }
                Some(Job::Create { slot, epoch, size }) => {
                    drop(state);
                    let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some("Video staging"),
                        size: u64::from(padded_row(size.0)) * u64::from(size.1),
                        usage: wgpu::BufferUsages::MAP_WRITE | wgpu::BufferUsages::COPY_SRC,
                        mapped_at_creation: true,
                    });
                    state = self.lock();
                    state.next_id += 1;
                    let id = state.next_id;
                    let slot = &mut state.slots[slot];
                    let made = Staging {
                        id,
                        buffer,
                        size,
                        stage: Stage::Mapped,
                        frame: 0,
                    };
                    if slot.epoch == epoch {
                        slot.staging.push(made);
                    } else {
                        // Its video was closed while it was made.
                        drop(state);
                        drop(made);
                        state = self.lock();
                    }
                }
                None => {
                    let open = |s: &Slot| s.pending.is_some() || !s.staging.is_empty();
                    if !state.slots.iter().any(open) {
                        break;
                    }
                    state = self
                        .wake
                        .wait(state)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
            }
        }
        state.running = false;
    }

    /// The staging buffer `id` of `slot` is mapped again (`mapped`), or never
    /// will be: its frame's commands were dropped, or the mapping failed.
    fn remapped(&self, slot: usize, id: u64, mapped: bool) {
        let mut state = self.lock();
        let staging = &mut state.slots[slot].staging;
        let mut gone = None;
        if let Some(i) = staging.iter().position(|s| s.id == id) {
            if mapped {
                staging[i].stage = Stage::Mapped;
            } else {
                gone = Some(staging.remove(i));
            }
        }
        drop(state);
        drop(gone);
        self.wake.notify_all();
    }
}

/// Copies the frame's rows into the mapped `buffer`, each padded to the copy
/// alignment. False when the buffer is not mapped.
fn copy(buffer: &wgpu::Buffer, frame: &VideoFrame) -> bool {
    let row = frame.width as usize * 4;
    let padded = padded_row(frame.width) as usize;
    let Ok(mut view) = buffer.slice(..).get_mapped_range_mut() else {
        return false;
    };
    if padded == row {
        // Rows need no padding (a 3840-wide 4K frame): one copy.
        view.slice(..frame.rgba.len()).copy_from_slice(&frame.rgba);
    } else {
        for (i, src) in frame.rgba.chunks_exact(row).enumerate() {
            view.slice(i * padded..i * padded + row)
                .copy_from_slice(src);
        }
    }
    true
}

/// Maps a staging buffer again after its frame is submitted, and says so
/// either way: dropped uncalled (the frame's encoder never was submitted), the
/// buffer is given up.
struct Remap {
    shared: Arc<Shared>,
    slot: usize,
    id: u64,
    told: bool,
}

impl Remap {
    fn finish(mut self, mapped: bool) {
        self.told = true;
        self.shared.remapped(self.slot, self.id, mapped);
    }
}

impl Drop for Remap {
    fn drop(&mut self) {
        if !self.told {
            self.shared.remapped(self.slot, self.id, false);
        }
    }
}

/// A slot's texture, the size of its video's frames, and the entity
/// resources with it in the atlas's place.
struct Target {
    texture: wgpu::Texture,
    group: wgpu::BindGroup,
}

/// The entity bind group with a video's texture as binding 1.
pub(crate) type MakeGroup = Box<dyn Fn(&wgpu::TextureView) -> wgpu::BindGroup + Send + Sync>;

pub(crate) struct VideoTextures {
    shared: Arc<Shared>,
    targets: [Option<Target>; VIDEO_SLOTS],
    make_group: MakeGroup,
}

impl VideoTextures {
    pub(crate) fn new(device: &wgpu::Device, make_group: MakeGroup) -> Self {
        Self {
            shared: Arc::new(Shared {
                device: device.clone(),
                state: Mutex::default(),
                wake: Condvar::new(),
            }),
            targets: Default::default(),
            make_group,
        }
    }

    /// The sampler of a video's texture: frames are scaled smoothly, and never wrap.
    pub(crate) fn sampler(device: &wgpu::Device) -> wgpu::Sampler {
        device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("Video"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        })
    }

    /// Takes `frame` as slot `slot`'s next picture, in place of one still
    /// waiting to be copied. Its pixels go back through `recycle` once copied,
    /// replaced or refused. False when it is refused: no such slot, or a frame
    /// whose size is not a video's or not its bytes'.
    pub(crate) fn submit(&self, slot: usize, frame: VideoFrame) -> bool {
        if slot >= VIDEO_SLOTS || !frame.valid() {
            frame.give_back();
            return false;
        }
        let mut state = self.shared.lock();
        if !state.running {
            // It waits for this lock, then finds the frame.
            let shared = self.shared.clone();
            let spawned = std::thread::Builder::new()
                .name("video-upload".into())
                .spawn(move || shared.run());
            match spawned {
                Ok(handle) => {
                    state.running = true;
                    // The thread before it, if any, has ended: it cleared `running`.
                    state.worker = Some(handle);
                }
                Err(e) => {
                    drop(state);
                    log::warn!("Horizon renderer: no video upload thread ({e})");
                    frame.give_back();
                    return false;
                }
            }
        }
        let replaced = state.slots[slot].pending.replace(frame);
        drop(state);
        self.shared.wake.notify_all();
        if let Some(frame) = replaced {
            frame.give_back();
        }
        true
    }

    /// Frees slot `slot`: its texture, its staging buffers and a frame still waiting.
    pub(crate) fn close(&mut self, slot: usize) {
        if slot >= VIDEO_SLOTS {
            return;
        }
        self.targets[slot] = None;
        let mut state = self.shared.lock();
        let slot = &mut state.slots[slot];
        slot.epoch += 1;
        let staging = std::mem::take(&mut slot.staging);
        let waiting = slot.pending.take();
        drop(state);
        drop(staging);
        self.shared.wake.notify_all();
        if let Some(frame) = waiting {
            frame.give_back();
        }
    }

    /// The entity resources that sample slot `slot`'s video; none before its
    /// first frame is in the texture.
    pub(crate) fn bind_group(&self, slot: usize) -> Option<&wgpu::BindGroup> {
        self.targets.get(slot)?.as_ref().map(|t| &t.group)
    }

    /// Whether slot `slot` has a picture, or a frame on its way to one. Not
    /// after `close`, nor in a renderer that was never given its frames.
    pub(crate) fn holds(&self, slot: usize) -> bool {
        if self.bind_group(slot).is_some() {
            return true;
        }
        let state = self.shared.lock();
        state
            .slots
            .get(slot)
            .is_some_and(|s| s.pending.is_some() || !s.staging.is_empty())
    }

    /// The newest copied frame of `slot`, taken for this frame's commands.
    fn take_ready(staging: &mut [Staging]) -> Option<(wgpu::Buffer, u64, (u32, u32))> {
        let newest = staging
            .iter()
            .filter(|s| s.stage == Stage::Ready)
            .map(|s| s.frame)
            .max()?;
        let mut taken = None;
        for s in staging.iter_mut().filter(|s| s.stage == Stage::Ready) {
            if s.frame == newest {
                s.stage = Stage::InFlight;
                taken = Some((s.buffer.clone(), s.id, s.size));
            } else {
                // An older frame nobody saw: its buffer is free again.
                s.stage = Stage::Mapped;
            }
        }
        taken
    }

    /// Once a frame, before the pass that draws the videos: each slot's newest
    /// copied frame goes from its staging buffer to its texture, in
    /// `encoder`'s order. The window's thread pays for recording the copy, not
    /// for the pixels (`video_slots::upload_micros`).
    pub(crate) fn prepare(&mut self, encoder: &mut wgpu::CommandEncoder) {
        let started = Instant::now();
        let mut copied = false;
        // One lock a frame for every slot, playing or not.
        let ready: [_; VIDEO_SLOTS] = {
            let mut state = self.shared.lock();
            std::array::from_fn(|slot| Self::take_ready(&mut state.slots[slot].staging))
        };
        for (slot, ready) in ready.into_iter().enumerate() {
            let Some((buffer, id, (width, height))) = ready else {
                continue;
            };
            let size = wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            };
            let fits = self.targets[slot]
                .as_ref()
                .is_some_and(|t| t.texture.size() == size);
            if !fits {
                self.targets[slot] = Some(self.target(size));
            }
            let Some(target) = &self.targets[slot] else {
                continue;
            };
            buffer.unmap();
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(padded_row(width)),
                        rows_per_image: Some(height),
                    },
                },
                wgpu::TexelCopyTextureInfo {
                    texture: &target.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                size,
            );
            let remap = Remap {
                shared: self.shared.clone(),
                slot,
                id,
                told: false,
            };
            encoder.map_buffer_on_submit(&buffer, wgpu::MapMode::Write, .., move |result| {
                remap.finish(result.is_ok());
            });
            copied = true;
        }
        if copied {
            crate::video_slots::uploaded(started);
        }
    }

    /// A slot's texture of `size`. `Rgba8Unorm`, as the atlas the videos were
    /// in: the world's frame keeps vanilla's gamma, so an sRGB format would
    /// darken the picture twice (docs/hdr-passes.md).
    fn target(&self, size: wgpu::Extent3d) -> Target {
        let mut usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
        if cfg!(test) {
            // The tests read the texture back.
            usage |= wgpu::TextureUsages::COPY_SRC;
        }
        let texture = self.shared.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("Video"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage,
            view_formats: &[],
        });
        let group = (self.make_group)(&texture.create_view(&Default::default()));
        Target { texture, group }
    }

    /// Slot `slot`'s texture, once it has one.
    #[cfg(test)]
    pub(crate) fn texture(&self, slot: usize) -> Option<&wgpu::Texture> {
        self.targets.get(slot)?.as_ref().map(|t| &t.texture)
    }

    /// Where each staging buffer of `slot` is.
    #[cfg(test)]
    pub(crate) fn stages(&self, slot: usize) -> Vec<Stage> {
        let state = self.shared.lock();
        state.slots[slot].staging.iter().map(|s| s.stage).collect()
    }

    /// Whether the copying thread is running.
    #[cfg(test)]
    pub(crate) fn running(&self) -> bool {
        self.shared.lock().running
    }
}

impl Drop for VideoTextures {
    fn drop(&mut self) {
        let mut state = self.shared.lock();
        state.stop = true;
        let mut staging = Vec::new();
        let waiting: Vec<VideoFrame> = state
            .slots
            .iter_mut()
            .filter_map(|s| {
                staging.append(&mut s.staging);
                s.pending.take()
            })
            .collect();
        let worker = state.worker.take();
        drop(state);
        drop(staging);
        self.shared.wake.notify_all();
        for frame in waiting {
            frame.give_back();
        }
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = crate::gfx::test_gpu::instance();
        let adapter = pollster::block_on(instance.request_adapter(&Default::default())).ok()?;
        pollster::block_on(adapter.request_device(&Default::default())).ok()
    }

    fn textures(device: &wgpu::Device) -> VideoTextures {
        let dev = device.clone();
        let make_group: MakeGroup = Box::new(move |view: &wgpu::TextureView| {
            let layout = dev.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries: &[wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                }],
            });
            dev.create_bind_group(&wgpu::BindGroupDescriptor {
                label: None,
                layout: &layout,
                entries: &[wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(view),
                }],
            })
        });
        VideoTextures::new(device, make_group)
    }

    fn pixels(width: u32, height: u32, seed: u8) -> Vec<u8> {
        (0..width * height * 4)
            .map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed))
            .collect()
    }

    fn frame(
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> (VideoFrame, std::sync::mpsc::Receiver<Vec<u8>>) {
        let (tx, rx) = std::sync::mpsc::channel();
        (
            VideoFrame {
                width,
                height,
                rgba,
                recycle: Box::new(move |v| {
                    let _ = tx.send(v);
                }),
            },
            rx,
        )
    }

    fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for: {what}");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn show(device: &wgpu::Device, queue: &wgpu::Queue, textures: &mut VideoTextures, slot: usize) {
        wait_until("stage Ready", || {
            textures.stages(slot).contains(&Stage::Ready)
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        textures.prepare(&mut encoder);
        queue.submit([encoder.finish()]);
    }

    fn read_back(device: &wgpu::Device, queue: &wgpu::Queue, texture: &wgpu::Texture) -> Vec<u8> {
        let width = texture.width();
        let height = texture.height();
        let padded = padded_row(width);
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: u64::from(padded) * u64::from(height),
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, |r| r.unwrap());
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        let data = buffer.slice(..).get_mapped_range().unwrap();
        let row = width as usize * 4;
        let mut out = Vec::with_capacity(row * height as usize);
        for y in 0..height as usize {
            let start = y * padded as usize;
            out.extend_from_slice(&data[start..start + row]);
        }
        out
    }

    /// Guards that `next_step` prefers a free mapped buffer, then creating a new one, then overwriting an untaken ready frame.
    #[test]
    fn the_copying_thread_prefers_a_free_buffer_then_a_new_one_then_an_untaken_frame() {
        let s = (100, 7);
        let other = (64, 4);
        assert_eq!(
            next_step(s, [(s, Stage::InFlight), (s, Stage::Mapped)].into_iter()),
            Step::Copy(1)
        );
        assert_eq!(next_step(s, std::iter::empty()), Step::Create);
        assert_eq!(next_step(s, [(s, Stage::Ready)].into_iter()), Step::Create);
        assert_eq!(
            next_step(s, [(s, Stage::InFlight), (s, Stage::Ready)].into_iter()),
            Step::Copy(1)
        );
        assert_eq!(
            next_step(s, [(s, Stage::InFlight), (s, Stage::InFlight)].into_iter()),
            Step::Wait
        );
        assert_eq!(
            next_step(
                s,
                [(other, Stage::Mapped), (other, Stage::Mapped)].into_iter()
            ),
            Step::Create
        );
    }

    /// Guards that `padded_row` rounds row byte counts up to `wgpu::COPY_BYTES_PER_ROW_ALIGNMENT`.
    #[test]
    fn a_staging_row_is_padded_to_the_copy_alignment() {
        assert_eq!(padded_row(3840), 15360);
        assert_eq!(padded_row(1), 256);
        assert_eq!(padded_row(100), 512);
        assert_eq!(padded_row(64), 256);
    }

    /// Guards that a submitted frame reaches the slot's texture, recycles its buffer, and remaps the staging buffer.
    #[test]
    fn a_frame_reaches_its_slots_texture_and_its_buffers_come_back() {
        let Some((device, queue)) = device() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let mut textures = textures(&device);
        let sent = pixels(100, 7, 7);
        let (f, back) = frame(100, 7, sent.clone());
        assert!(textures.submit(0, f));
        assert!(textures.bind_group(0).is_none());
        show(&device, &queue, &mut textures, 0);
        assert!(textures.bind_group(0).is_some());
        let texture = textures.texture(0).unwrap();
        assert_eq!((texture.width(), texture.height()), (100, 7));
        assert_eq!(texture.format(), wgpu::TextureFormat::Rgba8Unorm);
        assert_eq!(read_back(&device, &queue, texture), sent);
        assert_eq!(back.recv_timeout(Duration::from_secs(5)).unwrap(), sent);
        wait_until("the staging buffer is mapped again", || {
            let _ = device.poll(wgpu::PollType::Poll);
            let stages = textures.stages(0);
            stages.contains(&Stage::Mapped) && !stages.contains(&Stage::InFlight)
        });
    }

    /// Guards that submitting a different frame size recreates the slot's texture with the new dimensions and pixels.
    #[test]
    fn another_size_makes_the_texture_again() {
        let Some((device, queue)) = device() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let mut textures = textures(&device);
        let (f1, _) = frame(100, 7, pixels(100, 7, 1));
        assert!(textures.submit(0, f1));
        show(&device, &queue, &mut textures, 0);

        let second = pixels(64, 4, 2);
        let (f2, _) = frame(64, 4, second.clone());
        assert!(textures.submit(0, f2));
        show(&device, &queue, &mut textures, 0);

        let texture = textures.texture(0).unwrap();
        assert_eq!((texture.width(), texture.height()), (64, 4));
        assert_eq!(read_back(&device, &queue, texture), second);
    }

    /// Guards that dropping an unsubmitted command encoder after `prepare` does not stall subsequent frames in the slot.
    #[test]
    fn a_frame_whose_commands_were_dropped_does_not_stall_the_slot() {
        let Some((device, queue)) = device() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let mut textures = textures(&device);
        let (f1, _) = frame(100, 7, pixels(100, 7, 1));
        assert!(textures.submit(0, f1));
        wait_until("stage Ready", || textures.stages(0).contains(&Stage::Ready));
        {
            let mut encoder = device.create_command_encoder(&Default::default());
            textures.prepare(&mut encoder);
        }
        let second = pixels(100, 7, 9);
        let (f2, _) = frame(100, 7, second.clone());
        assert!(textures.submit(0, f2));
        show(&device, &queue, &mut textures, 0);
        let texture = textures.texture(0).unwrap();
        assert_eq!(read_back(&device, &queue, texture), second);
    }

    /// Guards that invalid slots, byte lengths, or oversized dimensions are refused and their pixel buffers returned.
    #[test]
    fn a_frame_that_is_not_a_videos_is_refused_and_given_back() {
        let Some((device, _queue)) = device() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let textures = textures(&device);
        let cases = [
            (VIDEO_SLOTS, frame(4, 4, pixels(4, 4, 0))),
            (0, frame(4, 4, vec![0; 60])),
            (
                0,
                frame(VIDEO_WIDTH + 1, 1, vec![0; (VIDEO_WIDTH as usize + 1) * 4]),
            ),
        ];
        for (slot, (f, back)) in cases {
            assert!(!textures.submit(slot, f));
            assert!(back.try_recv().is_ok());
        }
        assert!(!textures.holds(0));
    }

    /// Guards that closing a slot releases its resources and stops the copying thread when no slots remain open.
    #[test]
    fn a_closed_slot_holds_nothing_and_the_thread_ends() {
        let Some((device, queue)) = device() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let mut textures = textures(&device);
        let (f, _) = frame(100, 7, pixels(100, 7, 0));
        assert!(textures.submit(0, f));
        show(&device, &queue, &mut textures, 0);
        assert!(textures.holds(0));
        textures.close(0);
        assert!(textures.bind_group(0).is_none());
        assert!(!textures.holds(0));
        assert!(textures.stages(0).is_empty());
        wait_until("the copying thread ended", || !textures.running());
    }

    /// Guards that dropping `VideoTextures` returns any waiting frame's pixel buffer via `recycle`.
    #[test]
    fn a_waiting_frame_is_given_back_when_the_textures_are_dropped() {
        let Some((device, _queue)) = device() else {
            eprintln!("no GPU adapter: skipped");
            return;
        };
        let textures = textures(&device);
        let sent = pixels(100, 7, 3);
        let (f, back) = frame(100, 7, sent.clone());
        assert!(textures.submit(0, f));
        drop(textures);
        assert_eq!(
            back.recv_timeout(Duration::from_secs(5)).unwrap().len(),
            sent.len()
        );
    }
}
