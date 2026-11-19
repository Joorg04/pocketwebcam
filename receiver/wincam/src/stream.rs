//! The camera's one video stream: offers NV12 at a few sizes, and on every request fills a
//! sample from the [`Feed`]. Follows the structure of VCamSample's MediaStream
//! (github.com/smourier/VCamSample, MIT).

use std::sync::Mutex;

use windows::core::{implement, Interface, Ref, Result, BOOL, GUID, PCWSTR, PWSTR};
use windows::Win32::Foundation::E_POINTER;
use windows::Win32::Media::KernelStreaming::{IKsControl, IKsControl_Impl, KSIDENTIFIER, PINNAME_VIDEO_CAPTURE};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;

use crate::attributes::delegate_attributes;
use crate::feed::Feed;
use crate::not_supported;

/// Sizes offered to apps, largest first (the first is the default). All 30 fps NV12.
pub const SIZES: [(u32, u32); 3] = [(1920, 1080), (1280, 720), (640, 360)];
const FPS: u32 = 30;
/// One frame at 30 fps, in 100 ns units.
const FRAME_DURATION: i64 = 10_000_000 / FPS as i64;

#[implement(IMFMediaStream2, IKsControl, IMFAttributes)]
pub struct Stream {
    attrs: IMFAttributes,
    state: Mutex<State>,
}

struct State {
    queue: Option<IMFMediaEventQueue>,
    descriptor: Option<IMFStreamDescriptor>,
    source: Option<IMFMediaSource>,
    allocator: Option<IMFVideoSampleAllocatorEx>,
    stream_state: MF_STREAM_STATE,
    size: (u32, u32),
    feed: Feed,
}

delegate_attributes!(Stream_Impl);

impl Stream {
    pub fn new(index: u32) -> Result<Stream> {
        // SAFETY: plain Media Foundation calls on objects we own.
        unsafe {
            let mut attrs = None;
            MFCreateAttributes(&mut attrs, 4)?;
            let attrs = attrs.ok_or(windows::core::Error::from(E_POINTER))?;
            attrs.SetGUID(&MF_DEVICESTREAM_STREAM_CATEGORY, &PINNAME_VIDEO_CAPTURE)?;
            attrs.SetUINT32(&MF_DEVICESTREAM_STREAM_ID, index)?;
            attrs.SetUINT32(&MF_DEVICESTREAM_FRAMESERVER_SHARED, 1)?;
            attrs.SetUINT32(&MF_DEVICESTREAM_ATTRIBUTE_FRAMESOURCE_TYPES, MFFrameSourceTypes_Color.0 as u32)?;

            let types: Vec<Option<IMFMediaType>> =
                SIZES.iter().map(|&(w, h)| media_type(w, h).ok()).collect();
            let descriptor = MFCreateStreamDescriptor(index, &types)?;
            descriptor.GetMediaTypeHandler()?.SetCurrentMediaType(types[0].as_ref())?;
            Ok(Stream {
                attrs,
                state: Mutex::new(State {
                    queue: Some(MFCreateEventQueue()?),
                    descriptor: Some(descriptor),
                    source: None,
                    allocator: None,
                    stream_state: MF_STREAM_STATE_STOPPED,
                    size: SIZES[0],
                    feed: Feed::new(),
                }),
            })
        }
    }

    pub fn set_source(&self, source: IMFMediaSource) {
        self.state.lock().unwrap().source = Some(source);
    }

    pub fn descriptor(&self) -> Result<IMFStreamDescriptor> {
        self.state.lock().unwrap().descriptor.clone().ok_or(MF_E_SHUTDOWN.into())
    }

    pub fn stream_state(&self) -> MF_STREAM_STATE {
        self.state.lock().unwrap().stream_state
    }

    pub fn set_allocator(&self, allocator: IMFVideoSampleAllocatorEx) {
        self.state.lock().unwrap().allocator = Some(allocator);
    }

    /// Starts streaming in `media_type` (one of ours), or the current type if `None`.
    pub fn start(&self, media_type: Option<IMFMediaType>) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        let (Some(queue), Some(allocator), Some(descriptor)) = (s.queue.clone(), s.allocator.clone(), s.descriptor.clone())
        else {
            return Err(MF_E_SHUTDOWN.into());
        };
        // SAFETY: plain Media Foundation calls.
        unsafe {
            let media_type = match media_type {
                Some(t) => t,
                None => descriptor.GetMediaTypeHandler()?.GetCurrentMediaType()?,
            };
            let packed = media_type.GetUINT64(&MF_MT_FRAME_SIZE)?;
            s.size = ((packed >> 32) as u32, packed as u32);
            allocator.InitializeSampleAllocator(10, &media_type)?;
            queue.QueueEventParamVar(MEStreamStarted.0 as u32, &GUID::zeroed(), Default::default(), std::ptr::null())?;
        }
        s.stream_state = MF_STREAM_STATE_RUNNING;
        Ok(())
    }

    pub fn stop(&self) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        let (Some(queue), Some(allocator)) = (s.queue.clone(), s.allocator.clone()) else {
            return Err(MF_E_SHUTDOWN.into());
        };
        // SAFETY: plain Media Foundation calls.
        unsafe {
            allocator.UninitializeSampleAllocator()?;
            queue.QueueEventParamVar(MEStreamStopped.0 as u32, &GUID::zeroed(), Default::default(), std::ptr::null())?;
        }
        s.stream_state = MF_STREAM_STATE_STOPPED;
        Ok(())
    }

    pub fn shutdown(&self) {
        let mut s = self.state.lock().unwrap();
        if let Some(queue) = s.queue.take() {
            // SAFETY: shutting down our own queue.
            let _ = unsafe { queue.Shutdown() };
        }
        s.descriptor = None;
        s.source = None;
        s.allocator = None;
    }

    fn queue(&self) -> Result<IMFMediaEventQueue> {
        self.state.lock().unwrap().queue.clone().ok_or(MF_E_SHUTDOWN.into())
    }
}

/// NV12 at `width` × `height`, 30 fps, square pixels.
fn media_type(width: u32, height: u32) -> Result<IMFMediaType> {
    let ratio = |a: u32, b: u32| ((a as u64) << 32) | b as u64;
    // SAFETY: setting attributes on a new media type.
    unsafe {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_NV12)?;
        t.SetUINT64(&MF_MT_FRAME_SIZE, ratio(width, height))?;
        t.SetUINT64(&MF_MT_FRAME_RATE, ratio(FPS, 1))?;
        t.SetUINT64(&MF_MT_PIXEL_ASPECT_RATIO, ratio(1, 1))?;
        t.SetUINT32(&MF_MT_INTERLACE_MODE, MFVideoInterlace_Progressive.0 as u32)?;
        t.SetUINT32(&MF_MT_ALL_SAMPLES_INDEPENDENT, 1)?;
        t.SetUINT32(&MF_MT_DEFAULT_STRIDE, width)?;
        t.SetUINT32(&MF_MT_AVG_BITRATE, width * height * 12 * FPS)?;
        Ok(t)
    }
}

impl IMFMediaEventGenerator_Impl for Stream_Impl {
    fn GetEvent(&self, flags: MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS) -> Result<IMFMediaEvent> {
        // Never hold the lock here: a blocking GetEvent would stall everything else.
        let queue = self.queue()?;
        unsafe { queue.GetEvent(flags.0) }
    }
    fn BeginGetEvent(&self, callback: Ref<IMFAsyncCallback>, state: Ref<windows_core::IUnknown>) -> Result<()> {
        let queue = self.queue()?;
        unsafe { queue.BeginGetEvent(callback.as_ref(), state.as_ref()) }
    }
    fn EndGetEvent(&self, result: Ref<IMFAsyncResult>) -> Result<IMFMediaEvent> {
        let queue = self.queue()?;
        unsafe { queue.EndGetEvent(result.as_ref()) }
    }
    fn QueueEvent(&self, met: u32, extended: *const GUID, status: windows_core::HRESULT, value: *const PROPVARIANT) -> Result<()> {
        let queue = self.queue()?;
        unsafe { queue.QueueEventParamVar(met, extended, status, value) }
    }
}

impl IMFMediaStream_Impl for Stream_Impl {
    fn GetMediaSource(&self) -> Result<IMFMediaSource> {
        self.state.lock().unwrap().source.clone().ok_or(MF_E_SHUTDOWN.into())
    }

    fn GetStreamDescriptor(&self) -> Result<IMFStreamDescriptor> {
        self.descriptor()
    }

    fn RequestSample(&self, token: Ref<windows_core::IUnknown>) -> Result<()> {
        let mut s = self.state.lock().unwrap();
        let (Some(queue), Some(allocator)) = (s.queue.clone(), s.allocator.clone()) else {
            return Err(MF_E_SHUTDOWN.into());
        };
        let (width, height) = (s.size.0 as usize, s.size.1 as usize);
        // SAFETY: the buffer is locked for writing while we fill it, and is at least
        // `pitch` × `height` × 1.5 bytes (checked) as NV12 requires.
        unsafe {
            let sample = allocator.AllocateSample()?;
            sample.SetSampleTime(MFGetSystemTime())?;
            sample.SetSampleDuration(FRAME_DURATION)?;
            let buffer: IMF2DBuffer2 = sample.GetBufferByIndex(0)?.cast()?;
            let (mut scan0, mut pitch, mut start, mut len) = (std::ptr::null_mut(), 0i32, std::ptr::null_mut(), 0u32);
            buffer.Lock2DSize(MF2DBuffer_LockFlags_Write, &mut scan0, &mut pitch, &mut start, &mut len)?;
            let pitch = pitch.unsigned_abs() as usize;
            let needed = pitch * height * 3 / 2;
            if pitch >= width && !scan0.is_null() && (len as usize) >= needed {
                let dst = std::slice::from_raw_parts_mut(scan0, needed);
                s.feed.fill(dst, width, height, pitch);
            }
            buffer.Unlock2D()?;
            if let Some(token) = token.as_ref() {
                sample.SetUnknown(&MFSampleExtension_Token, token)?;
            }
            queue.QueueEventParamUnk(MEMediaSample.0 as u32, &GUID::zeroed(), Default::default(), &sample)?;
        }
        Ok(())
    }
}

impl IMFMediaStream2_Impl for Stream_Impl {
    fn SetStreamState(&self, value: MF_STREAM_STATE) -> Result<()> {
        let current = self.stream_state();
        if current == value {
            return Ok(());
        }
        match value {
            MF_STREAM_STATE_PAUSED if current == MF_STREAM_STATE_RUNNING => {
                self.state.lock().unwrap().stream_state = value;
                Ok(())
            }
            MF_STREAM_STATE_RUNNING => self.start(None),
            MF_STREAM_STATE_STOPPED => self.stop(),
            _ => Err(MF_E_INVALID_STATE_TRANSITION.into()),
        }
    }

    fn GetStreamState(&self) -> Result<MF_STREAM_STATE> {
        Ok(self.stream_state())
    }
}

impl IKsControl_Impl for Stream_Impl {
    fn KsProperty(&self, _: *const KSIDENTIFIER, _: u32, _: *mut core::ffi::c_void, _: u32, _: *mut u32) -> Result<()> {
        Err(not_supported())
    }
    fn KsMethod(&self, _: *const KSIDENTIFIER, _: u32, _: *mut core::ffi::c_void, _: u32, _: *mut u32) -> Result<()> {
        Err(not_supported())
    }
    fn KsEvent(&self, _: *const KSIDENTIFIER, _: u32, _: *mut core::ffi::c_void, _: u32, _: *mut u32) -> Result<()> {
        Err(not_supported())
    }
}
